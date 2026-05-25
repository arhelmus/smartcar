FLUTTER_DIR := server/flutter-ui

.PHONY: init dev review deploy

init:
	python3 scripts/init.py

dev:
	cd $(FLUTTER_DIR) && flutter run -d macos

# Same checks the pre-push hook runs (cargo fmt/clippy/test/audit, cross
# check on non-Linux, Flutter checks for mobile/ and flutter-ui/).
# Pass extra args via ARGS, e.g.  make review ARGS=--no-cross
review:
	python3 scripts/review.py $(ARGS)

# One-shot deploy: cross-build + rsync + ansible + restart + healthcheck.
# Requires: `python3 scripts/assign_board.py` ran first (sudo), and the
# board is in CAR mode. Flags pass through after `--` (make's own option
# parser needs the separator):
#   make deploy                       # full deploy (release)
#   make deploy -- --check            # ansible --check --diff, no restart
#   make deploy -- --skip-build       # use binary already on the board
# For args whose value contains spaces (--runtime-args "..."), shell
# quoting does not survive make's goal parsing — call the script directly:
#   python3 scripts/deploy.py --runtime-args "--log debug"
ifneq (,$(filter deploy,$(MAKECMDGOALS)))
  DEPLOY_ARGS := $(filter-out deploy,$(MAKECMDGOALS))
  # Stub no-op rules so make doesn't try to build the pass-through args.
  $(foreach w,$(DEPLOY_ARGS),$(eval $(w):;@:))
endif
deploy:
	python3 scripts/deploy.py $(DEPLOY_ARGS)
