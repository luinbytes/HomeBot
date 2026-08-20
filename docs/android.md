# Android architecture

The Android app is a native Kotlin/Compose client, never a second runtime. It uses the same HTTP/WebSocket protocol, stores its revocable device credential in Android Keystore, stores non-secret preferences in DataStore, and uses Room only for a replaceable offline cache.

The connection state machine covers unpaired, pairing, connecting, hydrating snapshot, live, reconnecting with cursor, version-incompatible, revoked, and offline-cache states. Feature modules render server-owned Bot, chat, group, activity, approval, routine, plugin, device, provider, workspace, diff, and Git models. Background reconnect must respect Android execution limits and avoid permanent polling.
