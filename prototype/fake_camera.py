#!/usr/bin/env python3
"""A stand-in for a Sony camera in Send to Smartphone mode, so the app can be tested without
the camera. It serves real JPEGs from a folder on this Mac, answering the way the A7 III did:
a device description, a ContentDirectory that lists date folders 50 entries at a time, and
thumbnail / small / large / original copies of each photo. The smaller copies are made with
macOS's `sips` the first time they're asked for.

    python3 prototype/fake_camera.py                       # photos from ~/Documents/SonyCamera
    python3 prototype/fake_camera.py --speed 0.4           # about as slow as the camera's Wi-Fi
    python3 prototype/fake_camera.py --photos ~/Pictures/test --port 64399

Then in the app, under Advanced, set the camera address to http://127.0.0.1:64321/dd.xml
(with the port you chose). It also works with sony_photos.py --camera <same address>.
"""

import argparse
import html
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PAGE = 50
CHUNK = 32 * 1024
# Longest side in pixels for each smaller copy. LRG roughly matches the A7 III's 2M copy.
SIZES = {"TN": 160, "SM": 640, "LRG": 1616}
# The A7 III doesn't always zero-pad its folder names ("2026-11-2"), so allow one or two digits.
DATE_FOLDER = re.compile(r"^(\d{4})-(\d{1,2})-(\d{1,2})$")

DIDL_OPEN = ('<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" '
             'xmlns:dc="http://purl.org/dc/elements/1.1/" '
             'xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">')


@dataclass
class Photo:
    number: int  # 1-based; used in this server's URLs
    path: str
    title: str
    taken: str  # 2026-10-24T11:37:11


def taken_at(path):
    """When the photo was taken, from its EXIF data via sips; the file's date if that fails."""
    try:
        out = subprocess.run(["sips", "-g", "creation", path], capture_output=True, text=True, timeout=10).stdout
        match = re.search(r"creation: (\d{4}):(\d{2}):(\d{2}) (\d{2}:\d{2}:\d{2})", out)
        if match:
            return f"{match[1]}-{match[2]}-{match[3]}T{match[4]}"
    except (OSError, subprocess.SubprocessError):
        pass
    return datetime.fromtimestamp(os.path.getmtime(path)).strftime("%Y-%m-%dT%H:%M:%S")


def load_photos(root):
    """Every JPEG under `root`, grouped into folders: the subfolder's name if it looks like a
    date (as the app saves them), otherwise the day each photo was taken."""
    folders = {}
    for dirpath, _, files in os.walk(root):
        for name in sorted(files):
            if not name.lower().endswith((".jpg", ".jpeg")):
                continue
            path = os.path.join(dirpath, name)
            taken = taken_at(path)
            parent = os.path.basename(dirpath)
            folder = parent if DATE_FOLDER.match(parent) else taken[:10]
            folders.setdefault(folder, []).append((name, path, taken))

    def by_date(folder):
        match = DATE_FOLDER.match(folder)
        return tuple(int(part) for part in match.groups()) if match else (0, 0, 0)

    photos, grouped = [], {}
    # By date rather than by name, so "2026-11-2" comes before "2026-11-10".
    for folder in sorted(folders, key=by_date):
        grouped[folder] = []
        for name, path, taken in sorted(folders[folder]):
            photo = Photo(len(photos) + 1, path, name, taken)
            photos.append(photo)
            grouped[folder].append(photo)
    return photos, grouped


class Throttle:
    """Limits all responses together to `mb_per_sec`, like the camera's one Wi-Fi link."""

    def __init__(self, mb_per_sec):
        self.rate = mb_per_sec * 1_000_000
        self.lock = threading.Lock()
        self.next_free = time.monotonic()

    def wait_for(self, size):
        if not self.rate:
            return
        with self.lock:
            now = time.monotonic()
            self.next_free = max(now, self.next_free) + size / self.rate
            delay = self.next_free - now
        time.sleep(delay)


class Copies:
    """Makes and keeps the smaller copies of each photo."""

    def __init__(self):
        self.dir = os.path.join(tempfile.gettempdir(), "sony-fake-camera-copies")
        os.makedirs(self.dir, exist_ok=True)
        self.lock = threading.Lock()

    def path(self, photo, size):
        if size == "ORG":
            return photo.path
        stamp = int(os.path.getmtime(photo.path))
        copy = os.path.join(self.dir, f"{photo.number}_{stamp}_{size}.jpg")
        with self.lock:
            if not os.path.exists(copy):
                result = subprocess.run(["sips", "-Z", str(SIZES[size]), photo.path, "--out", copy],
                                        capture_output=True, timeout=60)
                if result.returncode != 0:
                    return photo.path  # sips failed: send the original instead
        return copy


def make_handler(name, base, photos, grouped, copies, throttle):
    by_number = {p.number: p for p in photos}
    folder_ids = {f"F{i}": folder for i, folder in enumerate(grouped, 1)}

    description = f"""<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>{html.escape(name)}</friendlyName>
    <modelName>SonyImagingDevice</modelName>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
        <controlURL>/upnp/control/ContentDirectory</controlURL>
      </service>
    </serviceList>
  </device>
</root>"""

    def children(object_id):
        if object_id == "0":
            return ['<container id="PhotoRoot" parentID="0">'
                    '<dc:title>PhotoRoot</dc:title><upnp:class>object.container</upnp:class></container>']
        if object_id == "PhotoRoot":
            return [f'<container id="{fid}" parentID="PhotoRoot"><dc:title>{folder}</dc:title>'
                    '<upnp:class>object.container</upnp:class></container>'
                    for fid, folder in folder_ids.items()]
        if object_id in folder_ids:
            items = []
            for p in grouped[folder_ids[object_id]]:
                res = "".join(f'<res protocolInfo="http-get:*:image/jpeg:*">{base}/{s}_{p.number}.JPG</res>'
                              for s in ("TN", "SM", "LRG", "ORG"))
                items.append(f'<item id="{object_id}_{p.number}" parentID="{object_id}" restricted="1">'
                             f'<dc:title>{html.escape(p.title)}</dc:title><dc:date>{p.taken}</dc:date>'
                             f'<upnp:class>object.item.imageItem.photo</upnp:class>{res}</item>')
            return items
        return []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def send_body(self, body, content_type):
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            for start in range(0, len(body), CHUNK):
                chunk = body[start:start + CHUNK]
                throttle.wait_for(len(chunk))
                self.wfile.write(chunk)

        def do_GET(self):
            try:
                if self.path == "/dd.xml":
                    return self.send_body(description.encode(), "text/xml")
                match = re.fullmatch(r"/(TN|SM|LRG|ORG)_(\d+)\.JPG", self.path)
                photo = match and by_number.get(int(match[2]))
                if not photo:
                    return self.send_error(404)
                with open(copies.path(photo, match[1]), "rb") as f:
                    self.send_body(f.read(), "image/jpeg")
            except (BrokenPipeError, ConnectionResetError):
                pass  # the app stopped waiting (e.g. moved to another photo)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
            if "#Browse" not in self.headers.get("SOAPACTION", ""):
                return self.send_error(500)
            found = re.search(r"<ObjectID>(.*?)</ObjectID>", body)
            start = re.search(r"<StartingIndex>(\d+)</StartingIndex>", body)
            everything = children(html.unescape(found[1]) if found else "")
            first = int(start[1]) if start else 0
            page = everything[first:first + PAGE]
            result = html.escape(DIDL_OPEN + "".join(page) + "</DIDL-Lite>")
            envelope = ('<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">'
                        '<s:Body><u:BrowseResponse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">'
                        f"<Result>{result}</Result><NumberReturned>{len(page)}</NumberReturned>"
                        f"<TotalMatches>{len(everything)}</TotalMatches><UpdateID>1</UpdateID>"
                        "</u:BrowseResponse></s:Body></s:Envelope>")
            try:
                self.send_body(envelope.encode(), 'text/xml; charset="utf-8"')
            except (BrokenPipeError, ConnectionResetError):
                pass

    return Handler


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--photos", default="~/Documents/SonyCamera", help="folder of JPEGs to serve")
    ap.add_argument("--port", type=int, default=64321)
    ap.add_argument("--speed", type=float, default=0,
                    help="limit in MB/s for everything sent (0 = no limit; the A7 III managed about 0.4)")
    ap.add_argument("--name", default="ILCE-7M3 (fake)", help="camera name shown in the app")
    args = ap.parse_args()

    root = os.path.expanduser(args.photos)
    if not os.path.isdir(root):
        sys.exit(f"No such folder: {root}")
    print(f"Reading photos in {root}…")
    photos, grouped = load_photos(root)
    if not photos:
        sys.exit(f"No JPEGs found in {root}")

    base = f"http://127.0.0.1:{args.port}"
    handler = make_handler(args.name, base, photos, grouped, Copies(), Throttle(args.speed))
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler)
    speed = f"{args.speed} MB/s" if args.speed else "full speed"
    print(f"Serving {len(photos)} photos in {len(grouped)} folders at {speed}.")
    print(f"Camera address for the app: {base}/dd.xml   (Ctrl+C to stop)")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
