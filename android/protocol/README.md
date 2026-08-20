# Android protocol binding

`ProtocolV1.kt` is generated from the Rust-owned v1 contract tooling. Do not edit it by hand.

Regenerate and verify drift with:

```sh
cargo run -p homebot-protocol --example export_android
cargo run -p homebot-protocol --example export_android -- --check
```

The Android application will compile this binding with Kotlin serialization when its Gradle module is introduced in M4.
