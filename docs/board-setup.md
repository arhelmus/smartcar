# Orange Pi Zero 2W — Board Configuration

## Hardware

- **SoC**: Allwinner H618
- **Board**: Orange Pi Zero 2W (`xunlong,orangepi-zero2w`)
- **OS**: Armbian (Debian-based)
- **USB**: One OTG-capable port (`musb-hdrc.4.auto` / `5100000.usb`). The remaining USB ports are EHCI host-only and cannot act as gadgets.

## USB modes

The board has a single UDC (USB Device Controller). It can run in one of two modes, selected at boot by a hardware jumper on the 40-pin header:

| Header pins | PI16 reads | Mode |
|---|---|---|
| **No jumper** (default) | HIGH (internal pull-up) | **Dev mode** — USB Ethernet (`g_ether`), SSH at `10.55.0.1` |
| **Jumper: pin 37 → pin 39** | LOW | **Car mode** — AOAP gadget, `smartcar-server --transport usb` auto-starts |

The strap pin is **`PI16`** — physical **pin 37** on the 40-pin header. It is
`gpiochip1` (`300b000.pinctrl`) **line 272** = SoC bank I, pin 16; pin 39 next
to it is `GND`. (History: the script first said `PA6` — not bonded to this
header at all — then `PI13`/pin 7; the pin-7 joint on the hand-soldered header
was unreliable, so the strap was moved to pin 37/`PI16`. See `git log`.)

## Mode select wiring

```
40-pin header (top view, pin 1 at corner nearest USB-C / micro-SD;
the odd column contains pin 1):

  odd           even
  1 [3.3V]    [5V  ] 2
  ...
 35 [PI2 ]    [PC12] 36
 37 [PI16]    [PI4 ] 38   ← PI16 strap  (pin 37)
 39 [GND ]    [PI3 ] 40   ← GND         (pin 39)
```

Pins 37 and 39 are both in the **odd column**, one position apart (…35, 37,
39), so a standard 0.1" jumper cap will not bridge them — use a short jumper
**wire** from pin 37 (`PI16`) to pin 39 (`GND`) to select car mode. Remove it
to return to dev/Ethernet mode.

## How it works at boot

1. **`usb-mode.service`** runs early. It reads PI16 (`gpiochip1`, line 272) with an internal pull-up via `gpioget`. If the pin is LOW, it creates `/run/usb-car-mode`.

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
