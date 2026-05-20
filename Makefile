FLUTTER_DIR := server/flutter-ui

.PHONY: init dev review

init:
	python3 scripts/init.py

dev:
	cd $(FLUTTER_DIR) && flutter run -d macos

# Same checks the pre-push hook runs (cargo fmt/clippy/test/audit, cross
# check on non-Linux, Flutter checks for mobile/ and flutter-ui/).
# Pass extra args via ARGS, e.g.  make review ARGS=--no-cross
review:
	python3 scripts/review.py $(ARGS)
