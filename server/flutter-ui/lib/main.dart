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
/// The native Rust embedder registers texture id 0 and pushes decoded H.264
/// frames into it via [FlutterEngineMarkExternalTextureFrameAvailable].  The
/// [Texture] widget consumes those frames with zero extra copies.
class _VideoScreen extends StatelessWidget {
  const _VideoScreen();

  @override
  Widget build(BuildContext context) {
    return const Scaffold(
      backgroundColor: Colors.black,
      body: SizedBox.expand(
        child: Texture(textureId: 0),
      ),
    );
  }
}
