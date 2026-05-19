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

/// Full-screen Android Auto video stream with an interactive overlay.
///
/// Background: in embedded mode the [Texture] shows frames the Rust embedder
/// pushes; in debug mode a placeholder. On top sits a button + counter so
/// head-unit touches (relayed through the input channel) are visibly handled.
class _VideoScreen extends StatelessWidget {
  const _VideoScreen();

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      body: Stack(
        fit: StackFit.expand,
        children: [
          kDebugMode
              ? const _DevMock()
              : const SizedBox.expand(child: Texture(textureId: 0)),
          const _TapCounter(),
        ],
      ),
    );
  }
}

/// A button and a tap counter, centred on screen.
class _TapCounter extends StatefulWidget {
  const _TapCounter();

  @override
  State<_TapCounter> createState() => _TapCounterState();
}

class _TapCounterState extends State<_TapCounter> {
  int _count = 0;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            'Taps: $_count',
            style: const TextStyle(
              color: Colors.white,
              fontSize: 48,
              fontWeight: FontWeight.bold,
            ),
          ),
          const SizedBox(height: 24),
          ElevatedButton(
            onPressed: () => setState(() => _count++),
            style: ElevatedButton.styleFrom(
              backgroundColor: Colors.lightBlueAccent,
              foregroundColor: Colors.black,
              padding: const EdgeInsets.symmetric(
                horizontal: 48,
                vertical: 28,
              ),
              textStyle: const TextStyle(fontSize: 28),
            ),
            child: const Text('Tap me'),
          ),
        ],
      ),
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
