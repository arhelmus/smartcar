FLUTTER_DIR := server/flutter-ui

.PHONY: init dev review assign deploy

# ── Goal pass-through ─────────────────────────────────────────────────────
# Every target listed below forwards extra args to its underlying script.
# Make's option parser eats leading dashes, so flag-style args need the
# `--` end-of-options marker:
#   make deploy                       # full deploy
#   make deploy -- --check            # ansible --check --diff, no restart
#   make assign -- --watch            # poll continuously
#   make review -- --no-cross         # skip the slow Linux cross-check
# Caveat: quoted multi-word values (e.g. --runtime-args "--log debug")
# don't survive make's goal parsing — call the script directly for those:
#   python3 scripts/deploy.py --runtime-args "--log debug"
PASS_THROUGH_TARGETS := init dev review assign deploy
ifneq (,$(filter $(PASS_THROUGH_TARGETS),$(MAKECMDGOALS)))
  EXTRA_ARGS := $(filter-out $(PASS_THROUGH_TARGETS),$(MAKECMDGOALS))
  # Stub no-op rules so make doesn't try to build the args as goals.
  $(foreach w,$(EXTRA_ARGS),$(eval $(w):;@:))
endif

init:
	python3 scripts/init.py $(EXTRA_ARGS)

dev:
	cd $(FLUTTER_DIR) && flutter run -d macos $(EXTRA_ARGS)

# Same checks the pre-push hook runs (cargo fmt/clippy/test/audit, cross
# check on non-Linux, Flutter checks for mobile/ and flutter-ui/).
review:
	python3 scripts/review.py $(EXTRA_ARGS)

# Bring up the laptop's USB-Ethernet interface so the board is reachable.
# Requires sudo — the recipe runs sudo internally so an interactive prompt
# appears here (the script needs root for ip/ifconfig). One-shot by default;
# `make assign -- --watch` polls every 5s and binds when the board boots.
assign:
	sudo python3 scripts/assign_board.py $(EXTRA_ARGS)

# One-shot deploy: cross-build + rsync + ansible + restart + healthcheck.
# Requires `make assign` first and the board to be in CAR mode.
deploy:
	python3 scripts/deploy.py $(EXTRA_ARGS)
