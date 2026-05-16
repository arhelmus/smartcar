FLUTTER_DIR := server/flutter-ui

.PHONY: dev

dev:
	cd $(FLUTTER_DIR) && flutter run -d macos
