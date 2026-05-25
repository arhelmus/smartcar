# Orange Pi Zero 2W — Board Configuration

## Hardware

- **SoC**: Allwinner H618
- **Board**: Orange Pi Zero 2W (`xunlong,orangepi-zero2w`)
- **OS**: Armbian (Debian-based)
- **USB**: One OTG-capable port (`musb-hdrc.4.auto` / `5100000.usb`). The remaining USB ports are EHCI host-only and cannot act as gadgets.

## USB modes

The board has a single UDC (USB Device Controller). It can run in one of two modes, selected at boot by a hardware jumper on the 40-pin header. The jumper only affects the **USB gadget** (g_ether vs AOAP); whether `smartcar-server` runs and which transport it picks is set by `smartcar_transport` in `board/group_vars/all.yml`.

| Header pins | PI16 reads | USB gadget | What auto-starts with `smartcar_transport: bt` (default) | …with `smartcar_transport: usb` |
|---|---|---|---|---|
| **No jumper** (default) | HIGH (internal pull-up) | `g_ether` → SSH at `10.55.0.1` | `smartcar-server --transport bt` (BR/EDR + WiFi to the car) | Unit gated off — needs the jumper or `car-mode-once` |
| **Jumper: pin 37 → pin 39** | LOW | AOAP gadget — UDC empty until claimed | `smartcar-server --transport bt` still runs, but **SSH is lost** (no `g_ether`) — only useful if you have serial | `smartcar-server --transport usb` claims the UDC |

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

The boot is gated so that the *first* USB gadget bound to the UDC matches the
selected mode — there is no transient state where the host sees `g_ether`
appear and then disappear. (An earlier revision did exactly that: it loaded
`g_ether` unconditionally at `sysinit` and tore it down later in
`smartcar-server.service`'s `ExecStartPre`; production car HUs appear to
react to the "device removed" event by cutting Vbus before our AOAP gadget
can come up. The teardown script `release-udc.sh` is now gone — the new
sequence makes it dead weight. See `git log` for the rework.)

1. **`systemd-modules-load.service`** (stock systemd unit, runs in sysinit)
   reads `/etc/modules-load.d/usb-gadget.conf` and `modprobe`s `libcomposite`.
   That module registers the `usb_gadget` configfs subsystem, which is what
   creates `/sys/kernel/config/usb_gadget/` — the directory smartcar-server's
   `setup_gadget()` mkdirs under to define the AOAP accessory. **Loading
   `libcomposite` has no user-visible USB effect** (no gadget appears on the
   bus, no UDC is bound); it's pure kernel infrastructure. In dev mode
   `libcomposite` would also be pulled in transitively by `modprobe g_ether`
   in the next step — but in car mode g_ether is skipped and nothing else
   loads it, so we need this explicit load. Without it,
   `smartcar-server` exits at the very first `mkdir` with `ENOENT` because
   `/sys/kernel/config/usb_gadget/` doesn't exist.

2. **`usb-mode.service`** runs early. The script first checks for the
   one-shot software override `/var/lib/smartcar/car-mode-once`: if present
   it is deleted and `/run/usb-car-mode` is created unconditionally — that
   path is for SSH-driven Mac testing without touching the jumper, see
   "Forcing car mode without the jumper" below. Otherwise the script reads
   PI16 (`gpiochip1`, line 272) with an internal pull-up via `gpioget`; if
   the pin is LOW it creates `/run/usb-car-mode`.

3. **`g_ether-load.service`** runs `After=usb-mode.service` with
   `ConditionPathExists=!/run/usb-car-mode`. In **dev mode** the condition
   passes and it `modprobe`s `g_ether` — USB-Ethernet comes up, `usb0` gets
   `10.55.0.1/24`, SSH works over the USB cable. In **car mode** the service
   is skipped and `g_ether` is **never loaded**; the UDC stays empty.
   `/etc/modules-load.d/usb-gadget.conf` no longer autoloads `g_ether` at
   `sysinit` for exactly this reason — only `libcomposite` (step 1) which
   is harmless in both modes.

4. **Car mode** (file present): **`smartcar-server.service`** starts
   directly (no `ExecStartPre` — the UDC is already empty thanks to step 3,
   and the configfs root already exists thanks to step 1).
   `smartcar-server --transport usb --bridge ble` claims the empty UDC via
   configfs and brings up the AOAP accessory gadget; the BLE GATT bridge to
   the iOS app comes up in parallel. (The binary's default bridge mode is
   TCP, suitable for the dev Mac + iOS Simulator — the board unit overrides
   it.) The car sees a single USB device appear on its bus this boot —
   never two.

## Files on the board

| Path | Purpose |
|---|---|
| `/usr/local/sbin/usb-mode-select.sh` | Honours `/var/lib/smartcar/car-mode-once` one-shot override first; otherwise reads GPIO and creates `/run/usb-car-mode` if in car mode |
| `/var/lib/smartcar/car-mode-once` | One-shot trigger (touch + reboot) that forces car mode for the next boot only; consumed by `usb-mode-select.sh` before it commits |
| `/etc/systemd/system/usb-mode.service` | Oneshot service, runs `usb-mode-select.sh` at boot |
| `/etc/systemd/system/g_ether-load.service` | Loads `g_ether` only when **not** in car mode (`ConditionPathExists=!/run/usb-car-mode`) |
| `/etc/systemd/system/smartcar-server.service` | Starts `smartcar-server`, conditional on `/run/usb-car-mode` |
| `/etc/modules-load.d/usb-gadget.conf` | Loads `libcomposite` only (registers the `usb_gadget` configfs subsystem). Intentionally **excludes** `g_ether` — that's gated by `g_ether-load.service` |
| `/etc/modprobe.d/g_ether.conf` | Stable MAC addresses for `usb0` (board: `02:00:00:00:0a:01`, laptop: `02:00:00:00:0a:02`) |
| `/etc/systemd/network/usb0.network` | Static IP `10.55.0.1/24` on `usb0` |
| `/usr/local/bin/smartcar-server` | Deployed by `scripts/deploy.py` |

## Dev workflow

**Laptop-side**: set the USB-Ethernet interface the Mac sees to `10.55.0.2/24`.

```bash
# One-shot: cross-compile + rsync + ansible + systemd restart + healthcheck.
# Requires assign_board.py to have run first (sudo) and the board to be in CAR mode.
python3 scripts/deploy.py                        # full deploy (release)
python3 scripts/deploy.py --check                # ansible --check --diff, no restart
python3 scripts/deploy.py --skip-build           # use binary already on the board

# To use USB car mode manually without the jumper:
# SSH in, then:
modprobe -r g_ether
/usr/local/bin/smartcar-server --transport usb
```

### Forcing car mode without the jumper (`debug_usb_gadget.py`)

For Mac-side gadget testing it's awkward to fiddle with the header jumper
between every iteration. `usb-mode-select.sh` honours a one-shot software
override: drop `/var/lib/smartcar/car-mode-once` and reboot, and the board
will come up in car mode regardless of the GPIO. The script `rm`s the
trigger file before it commits, so the *next* boot reverts to whatever the
jumper actually says (i.e. dev mode if you left it off).

When the override path is taken, `usb-mode-select.sh` also schedules a
transient `systemd-run --on-active=30s -- /sbin/reboot` so the board
auto-reverts to dev mode after a 30 s car-mode window — no power-cycle
needed to get SSH back. (The transient unit is owned by PID 1 and
survives the oneshot exiting.)

```bash
# Helper that does the full cycle: trigger, wait for auto-revert,
# dump the flight log inline:
python3 scripts/debug_usb_gadget.py

# Or the raw one-liner if you just want to trigger and read logs by hand:
ssh root@10.55.0.1 'mkdir -p /var/lib/smartcar && touch /var/lib/smartcar/car-mode-once && reboot'
```

Workflow for a Mac-host USB iteration (no car, no jumper, no walk, no
power-cycle):

1. `python3 scripts/deploy.py --skip-build` — re-apply ansible + restart (or `python3 scripts/deploy.py` to rebuild first).
2. `python3 scripts/debug_usb_gadget.py` — trigger car-mode boot,
   wait ~60 s for the auto-revert, then print this iteration's
   `/var/log.hdd/smartcar-boot.log` section to the terminal.

If you accidentally leave the jumper *on* and use the override anyway,
the override fires for the boot it triggers (and auto-reverts), but the
auto-revert reboot then reads the jumper as LOW and lands you back in
car mode (no auto-revert this time, since the trigger file is gone).
Remove the jumper to get back to dev mode.

## Setup quirks (read before debugging weirdness)

A grab-bag of board-specific gotchas that have bitten us once and you'd
otherwise spend hours rediscovering.

### No hardware RTC

The H618 has no battery-backed RTC. The board boots at year 1970 unless
something restores the time. We use **`fake-hwclock`**:

- The package is installed.  `/etc/fake-hwclock.data` stores the last seen
  wall time; the **`fake-hwclock-load.service`** restores it at early boot
  (this is the split-service form on modern Debian).
- The **monolithic `fake-hwclock.service` is intentionally masked** — it's
  superseded by the split `fake-hwclock-load.service` + `-save.service` pair.
  Don't unmask it.
- To force a sync to current time (e.g. before a car trip):
  `date -u -s '<UTC time>' && fake-hwclock save`
- The wall clock matters for TLS certificate validity windows — a 1970
  clock will make any cert "not yet valid". Sync before driving.

### Journal persistence and aggressive flushing

`/var/log/journal → /var/log.hdd/journal` makes the systemd journal
persistent. `/etc/systemd/journald.conf` is tuned for **debugging unclean
power-loss in the car**:

- `Storage=auto` + `/var/log/journal` present ⇒ persistent.
- `SystemMaxUse=500M` — large enough to capture a long drive at debug level.
- **`SyncIntervalSec=0`** — every log line is `fsync`'d on write. Tiny
  perf cost; the difference between knowing why the car rejected us and
  finding a 1-second truncated boot in `--list-boots`.

Past-boot retrieval (boot IDs are stable even if timestamps are off):

```bash
journalctl --list-boots                      # find the trip's boot index
journalctl -u smartcar-server -b -1          # previous-boot session
journalctl -u usb-mode -b -1                 # was CAR mode triggered?
journalctl -k -b -1 | grep -iE 'musb|gadget|udc'  # kernel-side gadget log
```

### Car HU compatibility — USB descriptors

Real car head units **whitelist specific USB Vendor IDs**. Production AA
phones use Google's VID `0x18d1`; the openauto/aap-server historical
default of Huawei `0x12d1:0x107e` is rejected as "unsupported USB device"
by at least one HU we've tested.

We currently impersonate **Google `0x18d1:0x2d00`** (AOAP accessory)
directly with `Manufacturer="Google"`, `Product="Pixel 8 Pro"`. Because
`0x2d00` *is* the accessory persona, **we skip the AOAP mode-switch
handshake on the initial gadget** (no `12d1:107e` → `18d1:2d00`
re-enumeration). If you hit an HU that *requires* seeing the
mode-switch (some firmware does), the strings in
`server/crates/aap-transport/src/usb/gadget.rs` are the place to tweak —
revert to `0x18d1:0x4ee1` (Pixel MTP) and re-enable `wait_for_aoap`.

### `smartcar-server.service` directive placement

`StartLimitIntervalSec` and `StartLimitBurst` belong in the **`[Unit]`**
section, not `[Service]`. systemd silently ignores them otherwise and
restart-burst protection is off. (We've made this mistake; the fix is in
the deployed unit file.)

## Bluetooth — AAW car transport

Modern Audi MMI (2021+) and BMW iDrive (7+) head units **do not accept wired
Android Auto** — the only way in is **AAW** (Android Auto Wireless). On AAW
the phone (us, impersonated by the board) and the car negotiate WiFi
credentials over a Bluetooth RFCOMM channel, then the phone joins the car's
hotspot and TCP-connects to the car on port 5288. Once the TCP socket is
open, the protocol above the transport is byte-identical to the openauto/USB
path — `aap-core` and the rest of the stack do not change.

This is what `--transport bt` does. Crate layout: `aap-transport/src/bt/`
(see the file-level rustdoc there for the wire details).

### What plays which role

| Role | Owner |
|---|---|
| BR/EDR RFCOMM **client** | Board (us). UUID `4de17a00-52cb-11e6-bdf4-0800200c9a66`, channel 8. |
| RFCOMM **server** | Car head unit. We connect outward; it accepts. |
| WiFi AP | Car head unit. SSID + WPA2 PSK delivered to us in `WifiInfoResponse`. |
| WiFi STA | Board. Joins the HU's AP via `wpa_supplicant` on `wlan0`. |
| AA TCP server (port 5288) | Car head unit. Address arrives in `WifiStartRequest`. |
| AA TCP client | Board. Outbound `connect()` from `wlan0`. |
| AA TLS server | Board (us) — unchanged from USB/TCP paths. |

The iOS-app BLE GATT bridge (`aap-bridge`, `--bridge ble`) is **disabled on
the board for the AAW build**: the combo radio is shared and the AAW
RFCOMM/WiFi work cannot tolerate `bluer` simultaneously holding the adapter
for a custom GATT advertisement. Board scripts default to `--bridge none`.

### Bring-up sequence

1. **First-time pairing (user, once per car)** — Pairing is **car-initiated**.
   `smartcar-server --transport bt` on the board makes the adapter
   discoverable with `Class=0x6c020c` (phone) and registers a Just Works
   agent that auto-accepts any incoming pair request. On the car: open
   **Android Auto / CarPlay → Add new device** (path varies by HU), the
   car scans BR/EDR, lists `smartcar`, the operator selects it, the car
   sends the pair request, BlueZ accepts via the agent, the bond is
   cached. No `bluetoothctl pair` on the board, no BD_ADDR to type, no
   env var to set.

2. **Every subsequent boot (automatic)** — `smartcar-server --transport bt`
   does:
   1. Open BlueZ adapter; make discoverable + register the Just Works agent
      (no-op if the bond already exists; the agent costs nothing).
   2. Scan paired devices for one whose SDP UUIDs contain the AAWG profile
      (`4de17a00-…`). On a previously-paired board this resolves in <1 s.
   3. `Device.Connect()` warm-up (best-effort, non-fatal).
   4. RFCOMM client `connect()` to the paired peer on channel 8.
   5. Receive `WifiStartRequest{ip, port}` from HU — save its WiFi-side
      AA-server address.
   6. Send `WifiInfoRequest{}` (empty) → receive `WifiInfoResponse{ssid,
      password, bssid, security, ap_type}`.
   7. Send `WifiStartResponse{status=SUCCESS}` and `WifiConnectionStatus{
      status=SUCCESS}` — RFCOMM is now drained.
   8. Write `/run/aaw-wpa.conf` with the HU's creds; `wpa_supplicant -B -i
      wlan0 -c /run/aaw-wpa.conf`.
   9. `systemd-networkd` DHCPs `wlan0` from `wlan0.network`; we poll
      `ip -4 addr show wlan0` for a global address.
   10. `TcpStream::connect(<HU_IP>:<HU_PORT>)` → wrap in `TcpTransport` →
      hand to `Connection` (unchanged from the openauto/USB paths).

The whole bootstrap takes ~5–15 s on a paired car within range. Flight-log
checkpoints (`/var/log.hdd/smartcar-boot.log`) mirror the USB-mode breadcrumbs
so post-mortem diffing across a failed AAW vs. successful USB boot is easy.

### Files on the board

Installed by the ansible playbook at `board/` — `bluetooth` role for the
BlueZ + WiFi userland and `/etc/bluetooth/main.conf`, `network` role for
the systemd-networkd + wpa_supplicant configs, `smartcar_server` role for
the unit + defaults file:

| Path | Owning role | Purpose |
|---|---|---|
| `/etc/bluetooth/main.conf` | `bluetooth` | BlueZ tuned for AAW: `ControllerMode=dual`, `Class=0x6c020c` (phone), `JustWorksRepairing=always`, `FastConnectable=true`. Stock file backed up to `main.conf.dist` on first run. |
| `/etc/systemd/network/40-wlan0.network` | `network` | `DHCP=ipv4` on `wlan0`, link-local off, IPv6 RA off — the HU's DHCP server is the single source of truth. |
| `/etc/wpa_supplicant/wpa_supplicant-wlan0.conf` | `network` | Empty stub so the systemd unit doesn't fail-to-start; the real config (`/run/aaw-wpa.conf`) is written at AAW-handshake time by `smartcar-server`. Deployed with `force: false` so hand-edits survive re-provisioning. |
| `RUST_LOG` env on the unit | `smartcar_server` | Set via `Environment=` in the deployed `.service` file (driven by `smartcar_rust_log` in `group_vars`). One-shot extra `ExecStart=` args are layered on top via `/run/smartcar-deploy/runtime.env` (transient, tmpfs) — see `scripts/deploy.py --runtime-args`. |
| `/etc/systemd/system/smartcar-server.service` | `smartcar_server` | Transport-aware. For `bt`: `After=bluetooth.service systemd-networkd.service`, no jumper gate. For `usb`: the historical `Requires=usb-mode.service` + `ConditionPathExists=/run/usb-car-mode`. |

To deploy on a fresh board:

```bash
cd board
ansible-playbook site.yml --check --diff       # dry-run first
ansible-playbook site.yml                      # apply
```

To switch a board from USB to AAW, set `smartcar_transport: bt` in
`board/group_vars/all.yml` (or in inventory, per host) and re-run the
playbook. There is no BD_ADDR to configure — pair via the car's AA setup
on the first boot and the bond is cached automatically.

### systemd unit (`/etc/systemd/system/smartcar-server.service`)

On a board configured for AAW, the unit's `ExecStart=` and `EnvironmentFile=`
look like:

```ini
[Service]
EnvironmentFile=-/etc/default/smartcar-server
ExecStart=/usr/local/bin/smartcar-server --transport bt --bridge none
Restart=on-failure
```

The previous USB path remains valid for cars that still accept wired AA —
just swap `--transport bt` for `--transport usb` (and the
`/run/usb-car-mode` jumper logic continues to apply for that case). See the
"USB modes" section above.

### Package + rfkill prerequisites

The `bluetooth` role apt-installs (see `bluetooth_packages` in
`board/group_vars/all.yml`):

- `bluez`, `bluez-tools` — adapter + RFCOMM, `bluetoothctl` for pairing.
- `wpasupplicant` — STA-mode WiFi association.
- `iw`, `iproute2`, `rfkill` — link state polling and unblocking.

Critically **not** installed: `hostapd`, `dnsmasq`. The board is a STA, never
an AP, in the AAW path. (`aa-proxy-rs` and `aawgd` are HU-impersonators and
do need hostapd; we go the other way.)

### WiFi 5 GHz STA requirement

The combo radio shares its single 2.4 GHz antenna with BR/EDR. While the
RFCOMM handshake takes seconds and ends before WiFi comes up, modern AAW
HUs (BMW iDrive 7+, Audi MMI 2021+) bring their AP up on **5 GHz** to keep
their own BT coexistence sane. The board therefore **must be able to join a
5 GHz network as STA** — hosting 5 GHz is not required since the HU is the
AP. Verify the chipset's STA-mode 5 GHz capability after provisioning:

```bash
ssh root@10.55.0.1 'iw phy 2>&1 | grep -A1 "Band 2:"'
```

Look for `Frequencies:` under `Band 2:` listing 5180/5240/5500/etc. MHz.
If only Band 1 appears, the AAW path will not work with these cars on this
radio and a small USB WiFi adapter is required (the `aa-proxy-rs` README
maintains a list of cheap dongles that are known good).

### Coexistence note (revisited)

BR/EDR RFCOMM during the handshake (a few seconds, <1 KB/s) and 5 GHz STA
for the AA video stream live on different bands and do not contend. Older
boards/chipsets that only do 2.4 GHz WiFi will see latency hits on the AA
video path while any BR/EDR profile is active (e.g. if you keep the iOS-app
GATT bridge on by passing `--bridge ble`). The default `--bridge none` on
this build avoids that.

### Disabled: the old phone↔board BLE bridge

The `aap-bridge` crate's BLE GATT service (custom service UUID
`7c63a8ee-…`) is **not loaded** by the board build:
`smartcar_bridge: none` in `board/group_vars/all.yml` makes the deployed
`/etc/systemd/system/smartcar-server.service` pass `--bridge none`. The
crate stays in the workspace for the TCP-mode dev path on the Mac (so the
iOS Simulator can still exercise the control plane) and for any future
re-introduction, but the bt module specifically does not co-exist with it
at runtime.

## Viewing logs in car mode

SSH over USB-Ethernet is unavailable while in car mode. After the drive,
remove the jumper, reboot into dev mode, then read the previous boot:

```bash
ssh root@10.55.0.1 'journalctl -u smartcar-server -b -1 --no-pager' > car-trip.log
```

The full debug filter (`RUST_LOG=info,aap_core=debug,aap_audio=debug,
aap_transport=debug`) is baked into the unit; expect handshake/audio/
transport detail.
