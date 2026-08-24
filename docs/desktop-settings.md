# Desktop settings and notifications

HomeBot desktop presents local application preferences in six grouped sections: General, Plugins, Appearance, Updates, Connection and Devices. Provider, plugin and device actions remain entry points to server-backed operations; the desktop does not become an independent authority for those records.

## Local preferences

The desktop persists a versioned settings document through eframe storage. It contains only non-secret UI preferences: notification topics, focus policy, launch-at-login preference, theme selection and safe connection/status display values. Provider credentials and device sessions are never persisted here.

Theme selection supports system, light and dark. System mode follows egui's native host-theme signal. Both explicit modes use the shared semantic token system. Text size can be adjusted from 80% to 200%, and Reduce interface motion removes sidebar and state-transition animation without changing application behavior.

Settings open as a viewport-bounded modal over the current workspace. Each section uses grouped cards, persistent local navigation and scrolling at compact window sizes so preferences do not displace the active conversation.

## Notifications and attention

The notification center consumes sequenced server events and deduplicates reconnect replay. It creates native macOS/Linux notifications for:

- completed Bot work;
- pending approval;
- failed activity or message.

Notifications are suppressed while HomeBot is focused unless the user opts in. When unfocused, the native window also requests informational or critical attention. Sidebar indicators distinguish working, approval and failure states while preserving unread state.

Every notification carries a structured `DeepLink` with the exact Bot, chat, message and activity identifiers available for that event. Native activation sends that structure back to the running app; no route is inferred from notification text.

The native notification service can be unavailable in a headless session. That failure is reported safely and never changes server history or read state.
