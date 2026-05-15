import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

void main() {
  runApp(const SmartcarApp());
}

class SmartcarApp extends StatelessWidget {
  const SmartcarApp({super.key});

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      debugShowCheckedModeBanner: false,
      home: _VideoScreen(),
    );
  }
}

/// Full-screen Android Auto video stream.
///
/// In release/embedded mode: [Texture] consumes frames pushed by the Rust
/// embedder via [FlutterEngineMarkExternalTextureFrameAvailable].
/// In debug mode: a placeholder is shown so the UI can be developed without
/// the Rust embedder running.
class _VideoScreen extends StatelessWidget {
  const _VideoScreen();

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      body: kDebugMode ? const _DevMock() : const SizedBox.expand(child: Texture(textureId: 0)),
    );
  }
}

class _DevMock extends StatelessWidget {
  const _DevMock();

  @override
  Widget build(BuildContext context) {
    return const Center(
      child: Text(
        'Android Auto — awaiting stream',
        style: TextStyle(color: Colors.white54, fontSize: 18),
      ),
    );
  }
}
