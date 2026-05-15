# smartcar architecture

`smartcar` plays the **projection source** role of the Android Auto protocol —
the side normally implemented by an Android phone running the Android Auto app.
It connects to a **head unit** which, for local development, is the
`openauto` emulator from opencardev.

Wire stack (bottom-up):

| Layer        | Crate           | Responsibility                                    |
|--------------|-----------------|---------------------------------------------------|
| Transport    | `aap-transport` | TCP/USB framing, multi-frame reassembly, TLS      |
| Codec        | `aap-proto`     | protobuf generated from AAProto                   |
| Control      | `aap-core`      | version, SSL, service discovery, channel mgmt     |
| Services     | `aap-video`, …  | per-channel logic (video sink, input source, …)   |
| Composition  | `smartcar-server` | bin: CLI, wiring, lifecycle                     |

All inter-crate coupling goes through `aap-contracts` (traits + POD types).
