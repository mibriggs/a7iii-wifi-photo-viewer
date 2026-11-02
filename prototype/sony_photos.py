#!/usr/bin/env python3
"""Browse and download photos from a Sony camera over Wi-Fi.

Uses the camera's "Send to Smartphone" mode, which runs a UPnP media server
(the same one Sony's Imaging Edge Mobile app talks to). Python 3.9+, no extra packages.

    python3 sony_photos.py list
    python3 sony_photos.py download 3 7 12 --out ~/Pictures/a7iii
    python3 sony_photos.py download --all
    python3 sony_photos.py gallery          # opens a browser gallery on http://127.0.0.1:8765
"""

import argparse
import html
import json
import os
import socket
import sys
import time
import urllib.parse
import urllib.request
import webbrowser
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Where Sony cameras put the device description when they act as the Wi-Fi access point.
DEFAULT_DESCRIPTION_URL = "http://192.168.122.1:64321/dd.xml"
CDS_TYPE = "urn:schemas-upnp-org:service:ContentDirectory:1"
TIMEOUT = 15

NS = {
    "dev": "urn:schemas-upnp-org:device-1-0",
    "didl": "urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/",
    "dc": "http://purl.org/dc/elements/1.1/",
    "upnp": "urn:schemas-upnp-org:metadata-1-0/upnp/",
}

# Sony marks each resource URL with its size: original, large (~2MP), small, thumbnail.
SIZE_PREFIXES = ("ORG_", "LRG_", "SM_", "TN_")


@dataclass
class Photo:
    index: int
    title: str
    date: str
    folder: str
    urls: dict = field(default_factory=dict)  # "ORG" / "LRG" / "SM" / "TN" -> URL

    def url(self, size):
        """The URL for the size asked for, falling back to the next-best one."""
        order = ["ORG", "LRG", "SM", "TN"]
        start = order.index(size)
        for s in order[start:] + order[:start][::-1]:
            if s in self.urls:
                return self.urls[s]
        return None


# ---------- discovery ----------

def ssdp_discover(wait=3.0):
    """Ask the network for UPnP devices; return the Sony camera's description URL, or None."""
    msg = ("M-SEARCH * HTTP/1.1\r\n"
           "HOST: 239.255.255.250:1900\r\n"
           'MAN: "ssdp:discover"\r\n'
           "MX: 2\r\n"
           "ST: upnp:rootdevice\r\n\r\n").encode()
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    sock.settimeout(0.5)
    try:
        sock.sendto(msg, ("239.255.255.250", 1900))
        deadline = time.time() + wait
        while time.time() < deadline:
            try:
                data, _ = sock.recvfrom(4096)
            except socket.timeout:
                continue
            text = data.decode(errors="replace")
            if "SonyImagingDevice" not in text:
                continue
            for line in text.splitlines():
                if line.lower().startswith("location:"):
                    return line.split(":", 1)[1].strip()
    except OSError:
        pass
    finally:
        sock.close()
    return None


def find_content_directory(description_url):
    """Read the device description; return (camera name, ContentDirectory control URL)."""
    with urllib.request.urlopen(description_url, timeout=TIMEOUT) as r:
        root = ET.fromstring(r.read())
    name = root.findtext("dev:device/dev:friendlyName", default="Sony camera", namespaces=NS)
    for svc in root.iterfind(".//dev:service", NS):
        if svc.findtext("dev:serviceType", namespaces=NS) == CDS_TYPE:
            control = svc.findtext("dev:controlURL", namespaces=NS)
            return name, urllib.parse.urljoin(description_url, control)
    raise RuntimeError("The camera answered but has no ContentDirectory service. "
                       "Is it in Send to Smartphone > Select on Smartphone mode?")


# ---------- ContentDirectory browsing ----------

def browse(control_url, object_id, start=0, count=0):
    """One Browse call. Returns (DIDL-Lite root element, number returned, total matches)."""
    body = (
        '<?xml version="1.0" encoding="utf-8"?>'
        '<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" '
        's:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body>'
        f'<u:Browse xmlns:u="{CDS_TYPE}">'
        f"<ObjectID>{html.escape(object_id)}</ObjectID>"
        "<BrowseFlag>BrowseDirectChildren</BrowseFlag>"
        "<Filter>*</Filter>"
        f"<StartingIndex>{start}</StartingIndex>"
        f"<RequestedCount>{count}</RequestedCount>"
        "<SortCriteria></SortCriteria>"
        "</u:Browse></s:Body></s:Envelope>"
    ).encode()
    req = urllib.request.Request(control_url, data=body, method="POST", headers={
        "Content-Type": 'text/xml; charset="utf-8"',
        "SOAPACTION": f'"{CDS_TYPE}#Browse"',
    })
    with urllib.request.urlopen(req, timeout=60) as r:
        envelope = ET.fromstring(r.read())
    # The response elements aren't namespaced, so match on local names.
    fields = {el.tag.rsplit("}", 1)[-1]: el.text or "" for el in envelope.iter()}
    didl = ET.fromstring(fields.get("Result") or "<DIDL-Lite/>")
    return didl, int(fields.get("NumberReturned") or 0), int(fields.get("TotalMatches") or 0)


def browse_all(control_url, object_id):
    """All direct children of a container, paging until TotalMatches is reached."""
    children = []
    start = 0
    while True:
        didl, returned, total = browse(control_url, object_id, start)
        page = list(didl)
        children.extend(page)
        start += returned or len(page)
        if not page or start >= total:
            return children


def list_photos(control_url, on_progress=None):
    """Walk the whole tree from the root and return every image item as a Photo."""
    photos = []

    def walk(object_id, folder):
        for el in browse_all(control_url, object_id):
            tag = el.tag.rsplit("}", 1)[-1]
            title = el.findtext("dc:title", default="", namespaces=NS)
            if tag == "container":
                walk(el.get("id"), title)
            elif tag == "item":
                cls = el.findtext("upnp:class", default="", namespaces=NS)
                if not cls.startswith("object.item.imageItem"):
                    continue
                urls = {}
                for res in el.iterfind("didl:res", NS):
                    url = (res.text or "").strip()
                    name = urllib.parse.urlparse(url).path.rsplit("/", 1)[-1]
                    for prefix in SIZE_PREFIXES:
                        if name.startswith(prefix):
                            urls.setdefault(prefix.rstrip("_"), url)
                            break
                    else:
                        urls.setdefault("ORG", url)
                photos.append(Photo(len(photos) + 1, title,
                                    el.findtext("dc:date", default="", namespaces=NS),
                                    folder, urls))
                if on_progress:
                    on_progress(len(photos))

    walk("0", "")
    return photos


# ---------- download ----------

def download(photo, size, out_dir):
    url = photo.url(size)
    if not url:
        raise RuntimeError(f"{photo.title}: the camera gave no download URL")
    folder = os.path.join(out_dir, safe_name(photo.folder) if photo.folder else "")
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, safe_name(photo.title) or f"photo_{photo.index}.jpg")
    if os.path.exists(path):
        return path, False
    tmp = path + ".part"
    with urllib.request.urlopen(url, timeout=60) as r, open(tmp, "wb") as f:
        while chunk := r.read(1 << 16):
            f.write(chunk)
    os.replace(tmp, path)
    return path, True


def safe_name(name):
    return "".join(c for c in name if c not in '/\\:*?"<>|').strip(". ")


# ---------- gallery ----------

def run_gallery(camera_name, photos, size, out_dir, port):
    """A small local web page: thumbnails, a bigger preview on click, and download buttons.
    Only serves photos from the camera's own list, and only on this computer."""
    by_index = {p.index: p for p in photos}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def send(self, code, body, ctype="text/html; charset=utf-8"):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def photo_from_query(self):
            q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            try:
                return by_index[int(q.get("i", [""])[0])], q
            except (ValueError, KeyError):
                return None, q

        def do_GET(self):
            route = urllib.parse.urlparse(self.path).path
            if route == "/":
                return self.send(200, gallery_page(camera_name, photos).encode())
            if route == "/img":
                photo, q = self.photo_from_query()
                s = q.get("s", ["TN"])[0]
                if not photo or s not in ("TN", "SM", "LRG"):
                    return self.send(404, b"not found", "text/plain")
                try:
                    with urllib.request.urlopen(photo.url(s), timeout=60) as r:
                        return self.send(200, r.read(), r.headers.get("Content-Type", "image/jpeg"))
                except OSError as e:
                    return self.send(502, str(e).encode(), "text/plain")
            self.send(404, b"not found", "text/plain")

        def do_POST(self):
            if urllib.parse.urlparse(self.path).path != "/download":
                return self.send(404, b"not found", "text/plain")
            photo, _ = self.photo_from_query()
            if not photo:
                return self.send(404, b"not found", "text/plain")
            try:
                path, new = download(photo, size, out_dir)
                result = {"ok": True, "path": path, "new": new}
            except OSError as e:
                result = {"ok": False, "error": str(e)}
            self.send(200, json.dumps(result).encode(), "application/json")

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    url = f"http://127.0.0.1:{port}/"
    print(f"Gallery running at {url}  (downloads go to {out_dir}). Press Ctrl+C to stop.")
    webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


def gallery_page(camera_name, photos):
    tiles = "\n".join(
        f'<figure data-i="{p.index}"><img loading="lazy" src="/img?i={p.index}&s=TN" alt="">'
        f"<figcaption>{p.index}. {html.escape(p.title)}<br><small>{html.escape(p.date or p.folder)}</small>"
        f'<button onclick="dl(event,{p.index})">Download</button></figcaption></figure>'
        for p in photos)
    return f"""<!doctype html><meta charset="utf-8"><title>{html.escape(camera_name)} photos</title>
<style>
 body{{font:14px -apple-system,system-ui,sans-serif;margin:16px;background:#f6f6f6;color:#111}}
 header{{display:flex;gap:12px;align-items:center;margin-bottom:12px}}
 #grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(160px,1fr));gap:10px}}
 figure{{margin:0;background:#fff;border-radius:8px;overflow:hidden;box-shadow:0 1px 2px #0002}}
 figure img{{width:100%;aspect-ratio:3/2;object-fit:cover;display:block;cursor:zoom-in;background:#ddd}}
 figcaption{{padding:6px 8px}} small{{color:#666}}
 button{{margin-top:4px;font:inherit;padding:3px 8px;cursor:pointer}}
 .done button{{background:#d7f5d7}}
 #view{{position:fixed;inset:0;background:#000d;display:none;align-items:center;justify-content:center}}
 #view img{{max-width:95vw;max-height:95vh}}
</style>
<header><h2 style="margin:0">{html.escape(camera_name)}: {len(photos)} photos</h2></header>
<div id="grid">{tiles}</div>
<div id="view" onclick="this.style.display='none'"><img id="big" alt=""></div>
<script>
document.getElementById('grid').addEventListener('click', e => {{
  if (e.target.tagName !== 'IMG') return;
  const i = e.target.closest('figure').dataset.i;
  document.getElementById('big').src = '/img?i=' + i + '&s=LRG';
  document.getElementById('view').style.display = 'flex';
}});
async function dl(e, i) {{
  const b = e.target; b.disabled = true; b.textContent = 'Downloading…';
  const r = await (await fetch('/download?i=' + i, {{method: 'POST'}})).json();
  b.textContent = r.ok ? (r.new ? 'Saved' : 'Already saved') : 'Failed';
  b.title = r.ok ? r.path : r.error;
  if (r.ok) b.closest('figure').classList.add('done'); else b.disabled = false;
}}
</script>"""


# ---------- CLI ----------

def connect(args):
    url = args.camera
    if not url:
        print("Looking for the camera…", file=sys.stderr)
        url = ssdp_discover() or DEFAULT_DESCRIPTION_URL
    try:
        name, control = find_content_directory(url)
    except OSError as e:
        sys.exit(f"Couldn't reach the camera at {url} ({e}).\n"
                 "On the camera: MENU > Network > Send to Smartphone Func. > Send to Smartphone > "
                 "Select on Smartphone, then join the Wi-Fi network it shows (DIRECT-xxxx).")
    print(f"Connected to {name}. Reading the card…", file=sys.stderr)
    photos = list_photos(control, lambda n: print(f"\r  {n} photos found", end="", file=sys.stderr))
    print(file=sys.stderr)
    return name, photos


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--camera", help=f"device description URL (default: auto-discover, then {DEFAULT_DESCRIPTION_URL})")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list", help="list the photos on the card")
    d = sub.add_parser("download", help="download photos by number (from 'list'), or --all")
    d.add_argument("numbers", nargs="*", type=int)
    d.add_argument("--all", action="store_true")
    g = sub.add_parser("gallery", help="browse thumbnails in your web browser and download from there")
    g.add_argument("--port", type=int, default=8765)
    for p in (d, g):
        p.add_argument("--out", default=os.path.expanduser("~/Documents/SonyCamera"), help="download folder")
        p.add_argument("--size", choices=["ORG", "LRG"], default="ORG",
                       help="ORG = full original (default), LRG = ~2MP copy")
    args = ap.parse_args()

    name, photos = connect(args)

    if args.cmd == "list":
        for p in photos:
            print(f"{p.index:5}  {p.title:<20} {p.date or p.folder:<22} {'/'.join(sorted(p.urls))}")
        print(f"\n{len(photos)} photos on {name}.")
    elif args.cmd == "download":
        if not args.all and not args.numbers:
            sys.exit("Give photo numbers from 'list', or --all.")
        wanted = photos if args.all else [p for p in photos if p.index in set(args.numbers)]
        for i, p in enumerate(wanted, 1):
            path, new = download(p, args.size, os.path.expanduser(args.out))
            print(f"[{i}/{len(wanted)}] {'saved' if new else 'already have'} {path}")
    elif args.cmd == "gallery":
        run_gallery(name, photos, args.size, os.path.expanduser(args.out), args.port)


if __name__ == "__main__":
    main()
