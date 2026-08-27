# Remote access and device pairing

HomeBot listens on `127.0.0.1:7123` by default. Keep that default for a desktop-only installation. A non-loopback listener is refused unless the operator also sets `HOMEBOT_ALLOW_REMOTE=1`; startup then emits a security warning. Prefer a private Tailscale address. For an Internet-reachable or otherwise public endpoint, terminate HTTPS at a trusted reverse proxy and advertise its `https://` origin during pairing.

## Headless listener

```sh
export HOMEBOT_DEVICE_TOKEN="$(openssl rand -hex 32)"
export HOMEBOT_DATABASE="$HOME/.local/share/homebot/homebot.db"
export HOMEBOT_BIND="100.64.0.10:7123"
export HOMEBOT_ALLOW_REMOTE=1
homebot-server
```

Substitute the host's actual LAN or Tailscale address. Do not bind `0.0.0.0` unless the host firewall limits reachability. HomeBot's listener is HTTP; an explicitly configured public endpoint must be an HTTPS reverse proxy. Authentication and server-side capabilities still apply on private networks.

## Pairing lifecycle

Only the owner bearer can create a pairing offer:

```sh
curl --fail-with-body \
  -H "Authorization: Bearer $HOMEBOT_DEVICE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"request_id\":\"$(uuidgen)\",\"endpoint\":\"https://homebot.example.invalid\",\"allow_insecure_private_network\":false}" \
  http://127.0.0.1:7123/api/v1/pairing
```

The response contains a five-minute `homebot://pair` deep link. Its `hbpair_` credential and separate `hbproof_` native proof are single-use pairing material, not a permanent session. The Android client exchanges both values at the advertised endpoint, names the device, and receives an `hbds_` session once. HomeBot stores only SHA-256 digests of the pairing material. Secret-bearing responses use `Cache-Control: no-store`.

Plain HTTP advertisements are accepted automatically only for loopback. A private LAN or Tailscale HTTP endpoint requires `allow_insecure_private_network: true` and returns a visible warning. Custom/public advertisements require HTTPS. Endpoint credentials, paths, queries, and fragments are rejected. Browser exchange requires an exact `Origin` match and rejects a missing origin. Native exchange has no browser origin, so it must supply the separate proof from the deep link; mixed or incomplete provenance is rejected.

List and revoke sessions with the owner bearer:

```sh
curl --fail-with-body \
  -H "Authorization: Bearer $HOMEBOT_DEVICE_TOKEN" \
  http://127.0.0.1:7123/api/v1/devices

curl --fail-with-body \
  -H "Authorization: Bearer $HOMEBOT_DEVICE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"request_id\":\"$(uuidgen)\",\"idempotency_key\":\"$(uuidgen)\"}" \
  http://127.0.0.1:7123/api/v1/devices/DEVICE_UUID/revoke
```

Revocation takes effect for subsequent HTTP calls and terminates a live event stream at its next heartbeat. Device sessions cannot create pairing offers, list devices, or revoke peers. Failed provenance attempts are bounded per offer. Unknown-token attempts are throttled by the direct peer address and retained in a bounded digest-only emergency ledger, so random tokens from one client or reverse proxy cannot exhaust a valid offer's exchange capacity.

The desktop Devices settings screen uses these same APIs. It generates/copies the deep link, displays endpoint warnings and authoritative device state, and sends revocation through the server; it never treats its local projection as authority.

## OAuth callback reachability

Remote MCP OAuth returns the provider browser to `/api/v1/oauth/mcp/callback` on the HomeBot endpoint used by the native client. Loopback HTTP is valid when the browser is on the Mac. An Android browser cannot use the Mac's loopback address, and OAuth does not permit an ordinary private-LAN HTTP redirect, so Android-initiated MCP sign-in requires the paired HomeBot endpoint to be reachable over HTTPS. The callback accepts only a short-lived, random, single-use state created by an authenticated native request; it does not accept a HomeBot bearer or device session in the URL.
