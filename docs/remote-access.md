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

The response contains a five-minute `homebot://pair` deep link. Its `hbpair_` credential is single-use and is not a permanent session. The Android client exchanges it at the advertised endpoint, names the device, and receives an `hbds_` session once. HomeBot stores only SHA-256 token digests. Secret-bearing responses use `Cache-Control: no-store`.

Plain HTTP advertisements are accepted automatically only for loopback. A private LAN or Tailscale HTTP endpoint requires `allow_insecure_private_network: true` and returns a visible warning. Custom/public advertisements require HTTPS. Endpoint credentials, paths, queries, and fragments are rejected. When a browser supplies an `Origin`, exchange requires an exact match; native clients may omit it.

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

Revocation takes effect for subsequent HTTP calls and terminates a live event stream at its next heartbeat. Device sessions cannot create pairing offers, list devices, or revoke peers. Pairing exchange is rate-limited and failed origin attempts are durable across restart.

The desktop Devices settings screen uses these same APIs. It generates/copies the deep link, displays endpoint warnings and authoritative device state, and sends revocation through the server; it never treats its local projection as authority.
