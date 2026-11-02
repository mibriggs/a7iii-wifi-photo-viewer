mod cache;
mod wifi;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cache::ImageCache;
use serde::Serialize;
use tauri::http::{Response, StatusCode, header::CONTENT_TYPE};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Semaphore;
use wifi::{Camera, Image, Photo, Size};

/// The connected camera, the photo list read from its card, and images fetched from it so far.
struct Session {
    /// Changes on every connect, so image URLs from an earlier card are never reused.
    id: u64,
    camera: Camera,
    photos: Vec<Photo>,
    /// Thumbnails and previews. Small, so plenty fit.
    previews: ImageCache,
    /// Originals fetched for zooming, about 14 MB each from an A7 III.
    originals: ImageCache,
}

struct AppState {
    session: Mutex<Option<Arc<Session>>>,
    next_id: Mutex<u64>,
    /// Where downloads go; None means the default, Documents/SonyCamera.
    download_dir: Mutex<Option<PathBuf>>,
    // The camera's web server is small and its Wi-Fi slow, so limit requests to it. Previews for
    // the viewer have their own allowance so they don't queue behind a grid full of thumbnails.
    thumbnail_requests: Semaphore,
    preview_requests: Semaphore,
    original_requests: Semaphore,
}

impl AppState {
    fn new() -> Self {
        AppState {
            session: Mutex::new(None),
            next_id: Mutex::new(1),
            download_dir: Mutex::new(None),
            thumbnail_requests: Semaphore::new(3),
            preview_requests: Semaphore::new(2),
            original_requests: Semaphore::new(1),
        }
    }

    fn current(&self) -> Result<Arc<Session>, String> {
        self.session
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "Not connected to a camera.".to_string())
    }
}

/// The download folder and which photos on the card already have a file in it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Folder {
    download_dir: String,
    saved: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Connected {
    session_id: u64,
    camera_name: String,
    photos: Vec<Photo>,
    #[serde(flatten)]
    folder: Folder,
}

#[derive(Serialize)]
struct Downloaded {
    path: String,
    new: bool,
}

#[derive(Clone, Serialize)]
struct Progress {
    index: usize,
    received: u64,
    /// None when the camera didn't say how big the file is.
    total: Option<u64>,
}

/// Connects and reads the card. `address` is the camera's description URL; leave it empty to
/// search the network. `download_dir` is the folder chosen earlier, if any. Sends
/// "scan-progress" events with the number of photos found so far.
#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    address: Option<String>,
    download_dir: Option<String>,
) -> Result<Connected, String> {
    if let Some(dir) = download_dir.filter(|d| !d.trim().is_empty()) {
        *state.download_dir.lock().unwrap() = Some(PathBuf::from(dir));
    }
    let address = address
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty());
    let camera = Camera::connect(address.as_deref()).await?;
    let _ = app.emit("scan-progress", 0);
    let photos = camera
        .list_photos(|found| {
            if found % 10 == 0 {
                let _ = app.emit("scan-progress", found);
            }
        })
        .await?;

    let id = {
        let mut next = state.next_id.lock().unwrap();
        *next += 1;
        *next
    };
    let session = Arc::new(Session {
        id,
        camera,
        photos,
        previews: ImageCache::new(150_000_000),
        originals: ImageCache::new(120_000_000),
    });
    *state.session.lock().unwrap() = Some(session.clone());
    Ok(Connected {
        session_id: id,
        camera_name: session.camera.name.clone(),
        photos: session.photos.clone(),
        folder: folder_info(&session, download_dir_of(&app, &state)?).await,
    })
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>) {
    *state.session.lock().unwrap() = None;
}

/// Changes the download folder. Returns which photos already have a file in the new one.
#[tauri::command]
async fn set_download_dir(state: State<'_, AppState>, path: String) -> Result<Folder, String> {
    let dir = PathBuf::from(path);
    *state.download_dir.lock().unwrap() = Some(dir.clone());
    match state.current() {
        Ok(session) => Ok(folder_info(&session, dir).await),
        Err(_) => Ok(Folder {
            download_dir: dir.display().to_string(),
            saved: Vec::new(),
        }),
    }
}

/// Checks the download folder again, e.g. after files were added or deleted in Finder.
#[tauri::command]
async fn check_saved(app: AppHandle, state: State<'_, AppState>) -> Result<Folder, String> {
    let session = state.current()?;
    Ok(folder_info(&session, download_dir_of(&app, &state)?).await)
}

/// Saves the full-size original of photo `index` into the download folder, from memory if it
/// was fetched for zooming. Sends "download-progress" events while it runs.
#[tauri::command]
async fn download(
    app: AppHandle,
    state: State<'_, AppState>,
    index: usize,
) -> Result<Downloaded, String> {
    let session = state.current()?;
    let photo = photo_at(&session, index).ok_or("That photo isn't on the camera's list.")?;
    let dir = download_dir_of(&app, &state)?;
    let mut on_progress = progress_events(&app, "download-progress", index);
    let (path, new) = match session.originals.get((index, Size::Original)) {
        Some(image) => {
            let saved = wifi::save_bytes(photo, &dir, &image.bytes).await?;
            let size = image.bytes.len() as u64;
            on_progress(size, Some(size));
            saved
        }
        None => session.camera.download(photo, &dir, on_progress).await?,
    };
    Ok(Downloaded {
        path: path.display().to_string(),
        new,
    })
}

fn download_dir_of(app: &AppHandle, state: &AppState) -> Result<PathBuf, String> {
    if let Some(dir) = state.download_dir.lock().unwrap().clone() {
        return Ok(dir);
    }
    app.path()
        .document_dir()
        .map(|d| d.join("SonyCamera"))
        .map_err(|e| e.to_string())
}

async fn folder_info(session: &Session, dir: PathBuf) -> Folder {
    let mut saved = Vec::new();
    for photo in &session.photos {
        if tokio::fs::try_exists(wifi::saved_path(photo, &dir))
            .await
            .unwrap_or(false)
        {
            saved.push(photo.index);
        }
    }
    Folder {
        download_dir: dir.display().to_string(),
        saved,
    }
}

fn photo_at(session: &Session, index: usize) -> Option<&Photo> {
    index.checked_sub(1).and_then(|i| session.photos.get(i))
}

/// Sends `event` as bytes arrive: at most about ten times a second, and always for the end.
fn progress_events<'a>(
    app: &'a AppHandle,
    event: &'static str,
    index: usize,
) -> impl FnMut(u64, Option<u64>) + 'a {
    let mut last_sent: Option<Instant> = None;
    move |received, total| {
        let finished = total == Some(received);
        if finished || last_sent.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
            last_sent = Some(Instant::now());
            let _ = app.emit(
                event,
                Progress {
                    index,
                    received,
                    total,
                },
            );
        }
    }
}

/// Serves camera://localhost/<session>/<tn|sm|lrg|org>/<index> so the app's <img> tags can load
/// images from the camera. Originals (for zooming) come from the download folder instead when
/// the photo has been downloaded, and send "original-progress" events while they load.
async fn serve_image(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    match fetch_image(app, path).await {
        Ok(image) => Response::builder()
            .header(CONTENT_TYPE, image.content_type.as_str())
            .body(image.bytes.clone()),
        Err((status, message)) => Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "text/plain")
            .body(message.into_bytes()),
    }
    .unwrap_or_else(|_| Response::new(Vec::new()))
}

async fn fetch_image(app: &AppHandle, path: &str) -> Result<Arc<Image>, (StatusCode, String)> {
    let not_found = |what: &str| (StatusCode::NOT_FOUND, what.to_string());
    let mut parts = path.trim_start_matches('/').split('/');
    let (Some(session_id), Some(size), Some(index), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(not_found("bad image address"));
    };

    let state = app.state::<AppState>();
    let session = state.current().map_err(|e| not_found(&e))?;
    if session_id.parse::<u64>().ok() != Some(session.id) {
        return Err(not_found("image is from an earlier connection"));
    }
    let size = Size::parse(size).ok_or_else(|| not_found("bad size"))?;
    let index: usize = index.parse().map_err(|_| not_found("no such photo"))?;
    let photo = photo_at(&session, index).ok_or_else(|| not_found("no such photo"))?;
    let url = photo
        .url(size)
        .ok_or_else(|| not_found("the camera has no copy at that size"))?;

    let (cache, requests, timeout) = match size {
        Size::Original => {
            if let Ok(dir) = download_dir_of(app, &state) {
                if let Ok(bytes) = tokio::fs::read(wifi::saved_path(photo, &dir)).await {
                    return Ok(Arc::new(Image {
                        bytes,
                        content_type: "image/jpeg".to_string(),
                    }));
                }
            }
            (
                &session.originals,
                &state.original_requests,
                Duration::from_secs(300),
            )
        }
        Size::Thumbnail => (
            &session.previews,
            &state.thumbnail_requests,
            Duration::from_secs(30),
        ),
        Size::Small | Size::Large => (
            &session.previews,
            &state.preview_requests,
            Duration::from_secs(30),
        ),
    };
    cache
        .get_or_fetch((index, size), || async {
            let _turn = requests.acquire().await.map_err(|e| e.to_string())?;
            if size == Size::Original {
                session
                    .camera
                    .fetch(
                        url,
                        timeout,
                        progress_events(app, "original-progress", index),
                    )
                    .await
            } else {
                session.camera.fetch(url, timeout, |_, _| {}).await
            }
        })
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .register_asynchronous_uri_scheme_protocol("camera", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            tauri::async_runtime::spawn(async move {
                responder.respond(serve_image(&app, &path).await);
            });
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            set_download_dir,
            check_saved,
            download
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
