//! Talks to a Sony camera in "Send to Smartphone" mode over the camera's own Wi-Fi network.
//!
//! The camera runs a UPnP media server, the one Sony's Imaging Edge Mobile app uses. Sony doesn't
//! document it; the details come from alpha_sync (https://github.com/Dart120/alpha_sync), which
//! worked them out from that app's network traffic. In this mode the camera only offers JPEGs:
//! RAW files are converted to JPEG before they're sent.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use roxmltree::{Document, Node};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;

/// Where Sony cameras serve their device description when they are the Wi-Fi access point.
pub const DEFAULT_DESCRIPTION_URL: &str = "http://192.168.122.1:64321/dd.xml";
const CDS_TYPE: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";

/// The sizes the camera offers for each photo. It marks each URL's file name with one of
/// these prefixes: ORG_ (the original), LRG_ (about 2MP), SM_ and TN_ (thumbnail).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    Original,
    Large,
    Small,
    Thumbnail,
}

impl Size {
    const ORDER: [Size; 4] = [Size::Original, Size::Large, Size::Small, Size::Thumbnail];

    fn from_file_name(name: &str) -> Option<Size> {
        [
            ("ORG_", Size::Original),
            ("LRG_", Size::Large),
            ("SM_", Size::Small),
            ("TN_", Size::Thumbnail),
        ]
        .into_iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, size)| size)
    }

    /// Parses the short names used in the app's image URLs.
    pub fn parse(name: &str) -> Option<Size> {
        match name {
            "org" => Some(Size::Original),
            "lrg" => Some(Size::Large),
            "sm" => Some(Size::Small),
            "tn" => Some(Size::Thumbnail),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Photo {
    /// 1-based position in the camera's list; the app refers to photos by this number.
    pub index: usize,
    pub title: String,
    pub date: String,
    /// The camera groups photos into folders named by date.
    pub folder: String,
    #[serde(skip)]
    urls: Vec<(Size, String)>,
}

impl Photo {
    /// The URL for `size`, or the nearest size the camera offers: smaller ones first, then bigger.
    pub fn url(&self, size: Size) -> Option<&str> {
        let start = Size::ORDER.iter().position(|s| *s == size)?;
        Size::ORDER[start..]
            .iter()
            .chain(Size::ORDER[..start].iter().rev())
            .find_map(|want| {
                self.urls
                    .iter()
                    .find(|(s, _)| s == want)
                    .map(|(_, url)| url.as_str())
            })
    }
}

/// An image fetched for display.
pub struct Image {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

pub struct Camera {
    pub name: String,
    control_url: String,
    http: reqwest::Client,
}

impl Camera {
    /// Finds the camera and its photo service. `description_url` skips the network search,
    /// which is how the fake camera is used for testing.
    pub async fn connect(description_url: Option<&str>) -> Result<Camera, String> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let url = match description_url {
            Some(url) => url.to_string(),
            None => ssdp_discover(Duration::from_secs(3))
                .await
                .unwrap_or_else(|| DEFAULT_DESCRIPTION_URL.to_string()),
        };

        let unreachable = |e: reqwest::Error| {
            format!(
                "Couldn't reach the camera at {url} ({e}). Check the camera is on the \
                 Send to Smartphone › Select on Smartphone screen and this computer is on its \
                 DIRECT-xxxx Wi-Fi network."
            )
        };
        let xml = http
            .get(&url)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(unreachable)?
            .text()
            .await
            .map_err(unreachable)?;

        let (name, control_path) = parse_description(&xml)?;
        let control_url = reqwest::Url::parse(&url)
            .and_then(|base| base.join(&control_path))
            .map_err(|e| format!("The camera gave a bad service address ({e})"))?;
        Ok(Camera {
            name,
            control_url: control_url.to_string(),
            http,
        })
    }

    /// Walks every folder on the card and returns its photos. Videos are skipped.
    /// `on_progress` is called with the running count as photos are found.
    pub async fn list_photos(
        &self,
        mut on_progress: impl FnMut(usize),
    ) -> Result<Vec<Photo>, String> {
        let mut photos = Vec::new();
        let mut seen = HashSet::new();
        let mut to_visit = vec![("0".to_string(), String::new())];

        while let Some((id, folder)) = to_visit.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let mut folders = Vec::new();
            for entry in self.browse_all(&id).await? {
                match entry {
                    Entry::Folder { id, title } => folders.push((id, title)),
                    Entry::Photo { title, date, urls } => {
                        photos.push(Photo {
                            index: photos.len() + 1,
                            title,
                            date,
                            folder: folder.clone(),
                            urls,
                        });
                        on_progress(photos.len());
                    }
                }
            }
            // Reversed so folders are visited in the order the camera lists them.
            to_visit.extend(folders.into_iter().rev());
        }
        Ok(photos)
    }

    /// Fetches an image for display. `on_progress` is called after each chunk with the bytes
    /// received so far and the size, if the camera sent one.
    pub async fn fetch(
        &self,
        url: &str,
        timeout: Duration,
        mut on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<Image, String> {
        let mut response = self
            .http
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let total = response.content_length();
        let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            bytes.extend_from_slice(&chunk);
            on_progress(bytes.len() as u64, total);
        }
        Ok(Image {
            bytes,
            content_type,
        })
    }

    /// Saves the original into `<dir>/<date folder>/<file name>`. Returns the path and whether
    /// it was newly downloaded (false when the file was already there). `on_progress` is called
    /// after each chunk with the bytes received so far and the file size, if the camera sent one.
    pub async fn download(
        &self,
        photo: &Photo,
        dir: &Path,
        mut on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(PathBuf, bool), String> {
        let url = photo
            .url(Size::Original)
            .ok_or_else(|| format!("The camera gave no download address for {}", photo.title))?;
        let path = saved_path(photo, dir);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok((path, false));
        }

        let save_error = |e: std::io::Error| format!("Couldn't save {}: {e}", path.display());
        if let Some(folder) = path.parent() {
            tokio::fs::create_dir_all(folder)
                .await
                .map_err(save_error)?;
        }

        let mut response = self
            .http
            .get(url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("Download of {} failed: {e}", photo.title))?;

        // Write to a .part file and rename at the end, so an interrupted download never
        // looks like a finished photo.
        let mut part = path.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let mut file = tokio::fs::File::create(&part).await.map_err(save_error)?;
        let total = response.content_length();
        let mut received = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("Download of {} failed: {e}", photo.title))?
        {
            file.write_all(&chunk).await.map_err(save_error)?;
            received += chunk.len() as u64;
            on_progress(received, total);
        }
        file.flush().await.map_err(save_error)?;
        drop(file);
        tokio::fs::rename(&part, &path).await.map_err(save_error)?;
        Ok((path, true))
    }

    /// All direct children of a folder, asking again until the camera has sent them all.
    /// (It sends at most 50 at a time.)
    async fn browse_all(&self, object_id: &str) -> Result<Vec<Entry>, String> {
        let mut entries = Vec::new();
        let mut start = 0;
        loop {
            let page = self.browse(object_id, start).await?;
            entries.extend(page.entries);
            start += if page.returned > 0 {
                page.returned
            } else {
                page.child_count
            };
            if page.child_count == 0 || start >= page.total {
                return Ok(entries);
            }
        }
    }

    async fn browse(&self, object_id: &str, start: usize) -> Result<BrowsePage, String> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:Browse xmlns:u="{CDS_TYPE}"><ObjectID>{}</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>{start}</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>"#,
            xml_escape(object_id)
        );
        let xml = self
            .http
            .post(&self.control_url)
            .timeout(Duration::from_secs(60))
            .header(CONTENT_TYPE, r#"text/xml; charset="utf-8""#)
            .header("SOAPACTION", format!("\"{CDS_TYPE}#Browse\""))
            .body(body)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("The camera stopped answering while reading the card ({e})"))?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        parse_browse_response(&xml)
    }
}

#[derive(Debug, PartialEq)]
enum Entry {
    Folder {
        id: String,
        title: String,
    },
    Photo {
        title: String,
        date: String,
        urls: Vec<(Size, String)>,
    },
}

#[derive(Debug)]
struct BrowsePage {
    entries: Vec<Entry>,
    /// Everything the camera sent in this page, including videos we skip.
    child_count: usize,
    returned: usize,
    total: usize,
}

/// Saves an original that's already in memory (fetched for zooming) to where `download` would.
/// Returns the path and whether it was newly saved (false when the file was already there).
pub async fn save_bytes(
    photo: &Photo,
    dir: &Path,
    bytes: &[u8],
) -> Result<(PathBuf, bool), String> {
    let path = saved_path(photo, dir);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok((path, false));
    }
    let save_error = |e: std::io::Error| format!("Couldn't save {}: {e}", path.display());
    if let Some(folder) = path.parent() {
        tokio::fs::create_dir_all(folder)
            .await
            .map_err(save_error)?;
    }
    // Same .part-then-rename as `download`, so a half-written file never looks finished.
    let mut part = path.clone().into_os_string();
    part.push(".part");
    tokio::fs::write(&part, bytes).await.map_err(save_error)?;
    tokio::fs::rename(&part, &path).await.map_err(save_error)?;
    Ok((path, true))
}

/// Where `download` saves a photo: `<dir>/<date folder>/<file name>`.
pub fn saved_path(photo: &Photo, dir: &Path) -> PathBuf {
    let folder = match safe_name(&photo.folder) {
        name if name.is_empty() => dir.to_path_buf(),
        name => dir.join(name),
    };
    let file_name = match safe_name(&photo.title) {
        name if name.is_empty() => format!("photo_{}.jpg", photo.index),
        name => name,
    };
    folder.join(file_name)
}

/// Asks the network for UPnP devices and returns the Sony camera's description URL.
async fn ssdp_discover(wait: Duration) -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    let request = "M-SEARCH * HTTP/1.1\r\n\
                   HOST: 239.255.255.250:1900\r\n\
                   MAN: \"ssdp:discover\"\r\n\
                   MX: 2\r\n\
                   ST: upnp:rootdevice\r\n\r\n";
    socket
        .send_to(request.as_bytes(), "239.255.255.250:1900")
        .await
        .ok()?;

    let mut buf = [0u8; 4096];
    let listen = async {
        loop {
            let (len, _) = socket.recv_from(&mut buf).await.ok()?;
            let reply = String::from_utf8_lossy(&buf[..len]);
            if !reply.contains("SonyImagingDevice") {
                continue;
            }
            let location = reply.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("location")
                    .then(|| value.trim().to_string())
            });
            if location.is_some() {
                return location;
            }
        }
    };
    tokio::time::timeout(wait, listen).await.ok().flatten()
}

/// Returns the camera's name and the address of its ContentDirectory (photo list) service.
fn parse_description(xml: &str) -> Result<(String, String), String> {
    let doc = Document::parse(xml)
        .map_err(|e| format!("The camera's description isn't valid XML ({e})"))?;
    let name = doc
        .descendants()
        .find(|n| n.tag_name().name() == "friendlyName")
        .and_then(|n| n.text())
        .map(str::trim)
        .unwrap_or("Sony camera")
        .to_string();
    doc.descendants()
        .filter(|n| n.tag_name().name() == "service")
        .find(|service| child_text(*service, "serviceType") == CDS_TYPE)
        .map(|service| child_text(service, "controlURL"))
        .filter(|control| !control.is_empty())
        .map(|control| (name, control.to_string()))
        .ok_or_else(|| {
            "The camera answered but has no photo service. Is it on the \
             Send to Smartphone › Select on Smartphone screen?"
                .to_string()
        })
}

fn parse_browse_response(xml: &str) -> Result<BrowsePage, String> {
    let doc = Document::parse(xml)
        .map_err(|e| format!("The camera sent a reply that isn't valid XML ({e})"))?;
    // The response fields aren't namespaced, so match on the bare names.
    let field = |name: &str| {
        doc.descendants()
            .find(|n| n.tag_name().name() == name)
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let (entries, child_count) = parse_didl(&field("Result"))?;
    Ok(BrowsePage {
        entries,
        child_count,
        returned: field("NumberReturned").parse().unwrap_or(0),
        total: field("TotalMatches").parse().unwrap_or(0),
    })
}

/// Parses the DIDL-Lite list inside a Browse response: folders and photos.
/// Returns them along with the number of children the camera sent, skipped ones included.
fn parse_didl(xml: &str) -> Result<(Vec<Entry>, usize), String> {
    if xml.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let doc = Document::parse(xml)
        .map_err(|e| format!("The camera's photo list isn't valid XML ({e})"))?;
    let mut entries = Vec::new();
    let mut child_count = 0;
    for el in doc.root_element().children().filter(|n| n.is_element()) {
        child_count += 1;
        match el.tag_name().name() {
            "container" => {
                let id = el.attribute("id").unwrap_or("").to_string();
                if !id.is_empty() {
                    entries.push(Entry::Folder {
                        id,
                        title: child_text(el, "title").to_string(),
                    });
                }
            }
            "item" if child_text(el, "class").starts_with("object.item.imageItem") => {
                let mut urls: Vec<(Size, String)> = Vec::new();
                for res in el.children().filter(|n| n.tag_name().name() == "res") {
                    let url = res.text().unwrap_or("").trim();
                    if url.is_empty() {
                        continue;
                    }
                    let file_name = url
                        .split('?')
                        .next()
                        .unwrap_or(url)
                        .rsplit('/')
                        .next()
                        .unwrap_or("");
                    let size = Size::from_file_name(file_name).unwrap_or(Size::Original);
                    if !urls.iter().any(|(s, _)| *s == size) {
                        urls.push((size, url.to_string()));
                    }
                }
                entries.push(Entry::Photo {
                    title: child_text(el, "title").to_string(),
                    date: child_text(el, "date").to_string(),
                    urls,
                });
            }
            _ => {}
        }
    }
    Ok((entries, child_count))
}

fn child_text<'a>(node: Node<'a, '_>, name: &str) -> &'a str {
    node.children()
        .find(|c| c.tag_name().name() == name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Drops characters that aren't allowed in file names on macOS or Windows.
fn safe_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect::<String>()
        .trim_matches(|c| c == '.' || c == ' ')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
        <root xmlns="urn:schemas-upnp-org:device-1-0"><device>
          <friendlyName>ILCE-7M3</friendlyName>
          <serviceList>
            <service>
              <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
              <controlURL>/upnp/control/ConnectionManager</controlURL>
            </service>
            <service>
              <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
              <controlURL>/upnp/control/ContentDirectory</controlURL>
            </service>
          </serviceList>
        </device></root>"#;

    // A Browse reply as the camera sends it: the DIDL-Lite list is escaped inside <Result>.
    const BROWSE_REPLY: &str = r#"<?xml version="1.0"?>
        <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>
        <u:BrowseResponse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">
        <Result>&lt;DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/"&gt;
          &lt;container id="05" parentID="03"&gt;&lt;dc:title&gt;2026-10-24&lt;/dc:title&gt;&lt;upnp:class&gt;object.container&lt;/upnp:class&gt;&lt;/container&gt;
          &lt;item id="05_1"&gt;&lt;dc:title&gt;DSC03612.JPG&lt;/dc:title&gt;&lt;dc:date&gt;2026-10-24T11:37:11&lt;/dc:date&gt;&lt;upnp:class&gt;object.item.imageItem.photo&lt;/upnp:class&gt;
            &lt;res&gt;http://192.168.122.1:60151/TN_DSC03612.JPG&lt;/res&gt;
            &lt;res&gt;http://192.168.122.1:60151/LRG_DSC03612.JPG&lt;/res&gt;
            &lt;res&gt;http://192.168.122.1:60151/ORG_DSC03612.JPG&lt;/res&gt;
          &lt;/item&gt;
          &lt;item id="05_2"&gt;&lt;dc:title&gt;C0001.MP4&lt;/dc:title&gt;&lt;upnp:class&gt;object.item.videoItem&lt;/upnp:class&gt;&lt;/item&gt;
        &lt;/DIDL-Lite&gt;</Result>
        <NumberReturned>3</NumberReturned><TotalMatches>120</TotalMatches><UpdateID>1</UpdateID>
        </u:BrowseResponse></s:Body></s:Envelope>"#;

    #[test]
    fn finds_name_and_photo_service() {
        let (name, control) = parse_description(DESCRIPTION).unwrap();
        assert_eq!(name, "ILCE-7M3");
        assert_eq!(control, "/upnp/control/ContentDirectory");
    }

    #[test]
    fn description_without_photo_service_is_an_error() {
        let xml = DESCRIPTION.replace("ContentDirectory:1", "Something:1");
        assert!(parse_description(&xml).is_err());
    }

    #[test]
    fn reads_folders_and_photos_and_skips_videos() {
        let page = parse_browse_response(BROWSE_REPLY).unwrap();
        assert_eq!((page.child_count, page.returned, page.total), (3, 3, 120));
        assert_eq!(page.entries.len(), 2);
        assert_eq!(
            page.entries[0],
            Entry::Folder {
                id: "05".into(),
                title: "2026-10-24".into()
            }
        );
        let Entry::Photo { title, date, urls } = &page.entries[1] else {
            panic!("expected a photo")
        };
        assert_eq!(title, "DSC03612.JPG");
        assert_eq!(date, "2026-10-24T11:37:11");
        assert_eq!(urls.len(), 3);
    }

    #[test]
    fn picks_requested_size_or_nearest() {
        let page = parse_browse_response(BROWSE_REPLY).unwrap();
        let Entry::Photo { title, date, urls } = page.entries.into_iter().nth(1).unwrap() else {
            panic!()
        };
        let photo = Photo {
            index: 1,
            title,
            date,
            folder: String::new(),
            urls,
        };
        assert!(
            photo
                .url(Size::Original)
                .unwrap()
                .ends_with("/ORG_DSC03612.JPG")
        );
        assert!(
            photo
                .url(Size::Thumbnail)
                .unwrap()
                .ends_with("/TN_DSC03612.JPG")
        );
        // No SM_ copy: falls back to the next smaller size, the thumbnail.
        assert!(
            photo
                .url(Size::Small)
                .unwrap()
                .ends_with("/TN_DSC03612.JPG")
        );
    }

    #[test]
    fn empty_result_is_an_empty_page() {
        let reply = BROWSE_REPLY.replace(
            &BROWSE_REPLY
                [BROWSE_REPLY.find("<Result>").unwrap()..BROWSE_REPLY.find("</Result>").unwrap()],
            "<Result>",
        );
        let page = parse_browse_response(&reply).unwrap();
        assert!(page.entries.is_empty());
        assert_eq!(page.child_count, 0);
    }

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_name("2026/10:24"), "20261024");
        assert_eq!(safe_name("../DSC1.JPG"), "DSC1.JPG");
        assert_eq!(safe_name(".."), "");
    }
}
