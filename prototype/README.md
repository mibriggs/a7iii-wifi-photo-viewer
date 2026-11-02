# Sony A7 III Wi-Fi photo prototype

Checks whether your A7 III lets a computer list and download its photos over Wi-Fi, using the
same UPnP media server Sony's Imaging Edge Mobile app uses. The protocol details come from
[alpha_sync](https://github.com/Dart120/alpha_sync) (ISC license), which worked them out by
watching that app's network traffic. Sony doesn't document it.

Needs Python 3.9 or newer. Nothing else to install.

## Try it with your camera

1. On the camera: **MENU › Network › Send to Smartphone Func. › Send to Smartphone › Select on Smartphone**.
   The screen shows a network name (`DIRECT-xxxx`) and a password.
2. On your Mac, join that Wi-Fi network. You'll be off the internet while you're connected to it.
3. In this folder, run:

   ```bash
   python3 sony_photos.py list
   ```

4. If photos are listed, try downloading one, then the gallery:

   ```bash
   python3 sony_photos.py download 1
   ```

   ```bash
   python3 sony_photos.py gallery
   ```

Downloads go to `~/Documents/SonyCamera/<date>/`. Add `--out <folder>` to change that, or
`--size LRG` for smaller ~2MP copies. Files already downloaded are skipped.

## If it doesn't work

Copy the full error output. These are the things worth checking:

- **"Couldn't reach the camera"**: check the Mac is on the camera's `DIRECT-` network, and that
  the camera is still on the Select on Smartphone screen. It times out after a while.
- **Camera found but 0 photos**: the A7 III may organize its folders differently from the
  cameras alpha_sync was tested on. Run this and send me the output:

  ```bash
  curl -s http://192.168.122.1:64321/dd.xml
  ```

- **Only JPEGs show up, not RAW files**: probably a camera limit. Send to Smartphone has
  historically only offered JPEGs. Not confirmed for the A7 III.

## Testing without the camera

`fake_camera.py` imitates the camera's server on this computer, serving real JPEGs from a folder
(default `~/Documents/SonyCamera`). Subfolders named like `2026-10-24` become the camera's date
folders; other photos are grouped by the day they were taken. Smaller copies are made with
`sips` the first time they're asked for.

```bash
python3 prototype/fake_camera.py --photos ~/Documents/SonyCamera
```

Add `--speed 0.4` to send about as slowly as the A7 III's Wi-Fi did (MB/s, shared across all
requests), which is useful for seeing progress bars. It prints the camera address to use.

**With the app:** on the connect screen, open **Advanced** and set the camera address to
`http://127.0.0.1:64321/dd.xml`, then click Connect. The app remembers it; empty the box to
use the real camera again.

**With this prototype:**

```bash
python3 prototype/sony_photos.py --camera http://127.0.0.1:64321/dd.xml gallery
```

It only imitates what the A7 III did in testing, so check camera-specific behaviour (speed,
timeouts, unusual replies) on the real camera now and then.
