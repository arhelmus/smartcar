# Orange Pi Zero 2W — Board Configuration

## Hardware

- **SoC**: Allwinner H618
- **Board**: Orange Pi Zero 2W (`xunlong,orangepi-zero2w`)
- **OS**: Armbian (Debian-based)
- **USB**: One OTG-capable port (`musb-hdrc.4.auto` / `5100000.usb`). The remaining USB ports are EHCI host-only and cannot act as gadgets.

## USB modes

The board has a single UDC (USB Device Controller). It can run in one of two modes, selected at boot by a hardware jumper on the 26-pin header:

| Header pins | PA6 reads | Mode |
|---|---|---|
| **No jumper** (default) | HIGH (internal pull-up) | **Dev mode** — USB Ethernet (`g_ether`), SSH at `10.55.0.1` |
| **Jumper: pin 7 → pin 6** | LOW | **Car mode** — AOAP gadget, `smartcar-server --transport usb` auto-starts |

## Mode select wiring

```
26-pin header (top view, pin 1 at corner nearest USB-C):

 1  2
 3  4
 5  4
[6] GND      ← connect jumper here
[7] PA6      ← to here for car mode
 8  9
10 11
...
```

Short pins 6 and 7 together with a jumper cap or a wire to select car mode. Remove it to return to dev/Ethernet mode.

## How it works at boot

1. **`usb-mode.service`** runs early. It reads PA6 (`gpiochip1`, line 6) with an internal pull-up via `gpioget`. If the pin is LOW, it creates `/run/usb-car-mode`.

2. **Dev mode** (file absent): `g_ether` stays loaded. `usb0` comes up at `10.55.0.1/24`. SSH works over the USB cable from the laptop.

3. **Car mode** (file present): **`smartcar-server.service`** starts. Its `ExecStartPre` runs `release-udc.sh` which calls `modprobe -r g_ether` to free the UDC. Then `smartcar-server --transport usb` runs the AOAP two-persona handshake and claims the UDC via configfs.

## Files on the board

| Path | Purpose |
|---|---|
| `/usr/local/sbin/usb-mode-select.sh` | Reads GPIO, creates `/run/usb-car-mode` if in car mode |
| `/usr/local/sbin/release-udc.sh` | Unloads `g_ether` to free the UDC |
| `/etc/systemd/system/usb-mode.service` | Oneshot service, runs `usb-mode-select.sh` at boot |
| `/etc/systemd/system/smartcar-server.service` | Starts `smartcar-server`, conditional on `/run/usb-car-mode` |
| `/etc/modprobe.d/g_ether.conf` | Stable MAC addresses for `usb0` (board: `02:00:00:00:0a:01`, laptop: `02:00:00:00:0a:02`) |
| `/etc/systemd/network/usb0.network` | Static IP `10.55.0.1/24` on `usb0` |
| `/usr/local/bin/smartcar-server` | Deployed by `scripts/deploy_board.py` |

## Dev workflow

**Laptop-side**: set the USB-Ethernet interface the Mac sees to `10.55.0.2/24`.

```bash
# Deploy a new build to the board
python3 scripts/deploy_board.py          # cross-compile + rsync

# Run in TCP dev mode (laptop openauto, no USB cable to car)
python3 scripts/run_board.py --laptop-ip 10.55.0.2

# To use USB car mode manually without the jumper:
# SSH in, then:
modprobe -r g_ether
/usr/local/bin/smartcar-server --transport usb
```

## Viewing logs in car mode

Since SSH over USB is unavailable while in car mode, check logs after switching back to dev mode:

```bash
journalctl -u smartcar-server -n 100
journalctl -u usb-mode -n 20
```
