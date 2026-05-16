FLUTTER_DIR := server/flutter-ui

.PHONY: init dev

init:
	python3 scripts/init.py

dev:
	cd $(FLUTTER_DIR) && flutter run -d macos
