# Android protocol binding

`ProtocolV1.kt` is generated from the Rust-owned v1 contract tooling. Do not edit it by hand.

Regenerate and verify drift with:

```sh
cargo run -p homebot-protocol --example export_android
cargo run -p homebot-protocol --example export_android -- --check
```

The Android application compiles this binding directly with Kotlin serialization. The connection layer parses the Rust event envelope's flattened `kind` payload and decodes typed generated models from it; provider-native payloads never enter the client contract.
