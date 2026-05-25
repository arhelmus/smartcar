# Board provisioning

Ansible playbook that takes a freshly-flashed Armbian image on an Orange Pi
Zero 2W and brings it to the state described in `../docs/board-setup.md`.

## Scope

Owned by this playbook:
- USB gadget mode-select (`usb-mode-select.sh`, `usb-mode.service`,
  `g_ether-load.service`) and the `libcomposite` autoload / `g_ether` MAC
  pinning that gate it.
- USB-Ethernet network config (`usb0` -> `10.55.0.1/24`) and the WiFi-STA
  config for the AAW transport (`/etc/systemd/network/40-wlan0.network` +
  the `wpa_supplicant-wlan0.conf` stub).
- Persistent journal (`/var/log/journal` -> `/var/log.hdd/journal`) and the
  journald tuning for unclean-power-loss debugging in the car.
- `fake-hwclock` in the split-unit form, with the monolithic service masked.
- Bluetooth userland for the AAW car transport: `bluez` + `wpasupplicant`
  + `iw`/`rfkill`, `/etc/bluetooth/main.conf` tuned for AAW (class=phone,
  ControllerMode=dual), and `bluetooth.service` enabled. The stock BlueZ
  file is preserved at `/etc/bluetooth/main.conf.dist` on first run.
- `smartcar-server.service` unit definition. Transport-aware — see
  "Choosing the AA transport" below.

Not owned:
- The `smartcar-server` binary, `libflutter_engine.so`, or `flutter_assets/`
  — those are cross-compiled and pushed by `scripts/deploy.py` (phase 1).
- Armbian flashing, kernel, DTB, boot environment, U-Boot tweaks.
- Interactive BT pairing with the car (one-time, via `bluetoothctl pair`).

## Choosing the AA transport

The `smartcar_transport` group var (default `bt`) selects which AA
transport the deployed `smartcar-server.service` runs:

| Value | When to use | Unit shape |
|---|---|---|
| `bt` (default) | Car HUs that require **AAW** (Audi MMI 2021+, BMW iDrive 7+) — most modern cars. | `After=bluetooth.service systemd-networkd.service`; no jumper gate. Starts on every boot; keep the jumper **off** or USB-Ethernet SSH is lost. |
| `usb` | Car HUs that still accept **wired** AA. | `Requires=usb-mode.service`, `ConditionPathExists=/run/usb-car-mode` (jumper or one-shot override gates it). |
| `tcp` | Dev only — talks to openauto on a laptop. Not for the board. | `After=network-online.target`. |

There is **no BD_ADDR to configure** — the bt module auto-discovers the
paired AAW peer from BlueZ's bonded-device list, filtered by the AAWG SDP
UUID (`4de17a00-…`).

### One-time pairing (per car)

Pairing is **car-initiated**. On the first boot under `--transport bt`,
`smartcar-server`:

1. Makes the adapter discoverable, with `Class=0x6c020c` (phone) so the car
   recognises it as an AA-capable device.
2. Registers a Just Works agent so an inbound pair request is auto-accepted
   without any board-side UI.
3. Polls for a paired AAW peer every 5 s, logging
   `bt: no paired AAW peer yet — open the car's Android Auto Wireless
   setup and select Smartcar to pair` until one appears.

On the car:

1. Open **Settings → Android Auto / CarPlay → Add new device** (exact path
   varies by HU).
2. The car scans BR/EDR and lists `Smartcar`.
3. Tap it. The car shows a Just Works confirmation prompt; accept.

BlueZ caches the bond. Subsequent boots find the bonded peer in <1 s, no
operator action needed.

## Prerequisites

- `ansible-core` >= 2.14 on the laptop (`brew install ansible` or
  `pip install ansible-core`).
- The board reachable on its USB-Ethernet IP (default `10.55.0.1`) with
  your SSH key already in `/root/.ssh/authorized_keys`. First-time
  flashing and SSH bootstrap fall outside this playbook.

## Usage

Board addressing (`BOARD_HOST`, `BOARD_USER`, `BOARD_MAC`, `BOARD_MAC_DEV`)
comes from `../.env.local` — the same source the python run scripts use.

The blessed entry point is `scripts/deploy.py`, which runs ansible as
phase 2 of its pipeline (after cross-build + rsync, before restart +
healthcheck). See `python3 scripts/deploy.py --help`. Common shortcuts:

```bash
make deploy                       # full pipeline
make deploy -- --check            # ansible --check --diff, no build/restart
make deploy -- --skip-build       # use binary already on the board
```

(The `--` is `make`'s own "end of options" marker; needed before flags
starting with `-` so make doesn't try to interpret them as its own.)

Direct `ansible-playbook` invocations work too, but you have to load
`.env.local` first or the pre-task assert will fail:

```bash
cd board
set -a; . ../.env.local; set +a
ansible-playbook site.yml --check --diff
```

## Adding another board

Add an entry to `inventory.yml` under `boards.hosts:`:

```yaml
orangepi-rig2:
  ansible_host: 10.55.0.3
  ansible_user: root
```

Nothing else changes — everything is keyed by inventory.

## Safety notes

- The playbook never (re)starts `g_ether-load.service` inside a run —
  that would tear down `usb0` and kill our SSH session. Changes to gadget
  config take effect on the next reboot.
- `smartcar-server.service` is **enabled but not started** by the
  playbook. For `smartcar_transport=usb` it is gated by
  `ConditionPathExists=/run/usb-car-mode` and only fires when the jumper
  or `car-mode-once` override selects car mode at boot. For
  `smartcar_transport=bt` it comes up unconditionally on
  `multi-user.target`; the operator decides when to bring it up the first
  time after pairing.
- The `usb_gadget` role refuses to add `g_ether` to
  `/etc/modules-load.d/`. Real car head units react to the "device
  removed" event by cutting Vbus before our AOAP gadget can come up; the
  load is gated by `g_ether-load.service` instead. See
  `../docs/board-setup.md` for the full history.
- The `fake_hwclock` role keeps the monolithic `fake-hwclock.service`
  **masked**. Do not unmask it; the split `-load`/`-save` pair
  supersedes it on Debian trixie.
- The `bluetooth` role does **not** register the phone↔board GATT service
  from `aap-bridge` — the playbook deploys `smartcar_bridge=none` so the
  custom GATT advertisement can't compete with AAW for the combo radio.
  If a future build re-enables it, do it via inventory, not by hand-editing
  the unit on a board.
- The wpa_supplicant config at `/etc/wpa_supplicant/wpa_supplicant-wlan0.conf`
  is deployed with `force: false` so a hand-edited home-network PSK survives
  re-provisioning. The AAW path doesn't use it: `smartcar-server` writes
  its own `/run/aaw-wpa.conf` at runtime.

## Re-provisioning

Designed to be re-run any time. All file copies use checksum-based
idempotency; systemd state is asserted, not blindly toggled. Cruft files
left behind by earlier hand-edits (`*.bak`, `*.bak-pa6`, `*.bak-pi13`,
AppleDouble `._*` files) are intentionally **not** cleaned up — they're
harmless and removing them is a one-time operator task, not something a
provisioning run should silently do.
