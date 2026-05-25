# Board provisioning

Ansible playbook that takes a freshly-flashed Armbian image on an Orange Pi
Zero 2W and brings it to the state described in `../docs/board-setup.md`.

## Scope

Owned by this playbook:
- USB gadget mode-select (`usb-mode-select.sh`, `usb-mode.service`,
  `g_ether-load.service`) and the `libcomposite` autoload / `g_ether` MAC
  pinning that gate it.
- USB-Ethernet network config (`usb0` -> `10.55.0.1/24`).
- Persistent journal (`/var/log/journal` -> `/var/log.hdd/journal`) and the
  journald tuning for unclean-power-loss debugging in the car.
- `fake-hwclock` in the split-unit form, with the monolithic service masked.
- `smartcar-server.service` unit definition.

Not owned:
- The `smartcar-server` binary, `libflutter_engine.so`, or `flutter_assets/`
  — those are cross-compiled and pushed by `scripts/deploy_board.py`.
- Armbian flashing, kernel, DTB, boot environment, U-Boot tweaks.
- Bluetooth bring-up (planned — see `../docs/board-setup.md` §Bluetooth).
  The `bluetooth` role exists as a stub for that work.

## Prerequisites

- `ansible-core` >= 2.14 on the laptop (`brew install ansible` or
  `pip install ansible-core`).
- The board reachable on its USB-Ethernet IP (default `10.55.0.1`) with
  your SSH key already in `/root/.ssh/authorized_keys`. First-time
  flashing and SSH bootstrap fall outside this playbook.

## Usage

```bash
cd board

# Smoke-test SSH reachability first.
ansible -m ping boards

# Dry-run with diffs to see what would change.
ansible-playbook site.yml --check --diff

# Apply.
ansible-playbook site.yml

# One specific host (when you have multiple boards).
ansible-playbook site.yml -l orangepi-dev

# Re-run just one role.
ansible-playbook site.yml --tags usb_gadget
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
  playbook; it is gated by `ConditionPathExists=/run/usb-car-mode` and
  only fires when the jumper or `car-mode-once` override selects car
  mode at boot.
- The `usb_gadget` role refuses to add `g_ether` to
  `/etc/modules-load.d/`. Real car head units react to the "device
  removed" event by cutting Vbus before our AOAP gadget can come up; the
  load is gated by `g_ether-load.service` instead. See
  `../docs/board-setup.md` for the full history.
- The `fake_hwclock` role keeps the monolithic `fake-hwclock.service`
  **masked**. Do not unmask it; the split `-load`/`-save` pair
  supersedes it on Debian trixie.

## Re-provisioning

Designed to be re-run any time. All file copies use checksum-based
idempotency; systemd state is asserted, not blindly toggled. Cruft files
left behind by earlier hand-edits (`*.bak`, `*.bak-pa6`, `*.bak-pi13`,
AppleDouble `._*` files) are intentionally **not** cleaned up — they're
harmless and removing them is a one-time operator task, not something a
provisioning run should silently do.
