# Updates, migration backup, and recovery

HomeBot treats application updates and database migrations as separate, explicit safety boundaries.

## Desktop updates

The desktop never downloads or installs an update merely because an update exists. Pressing **Check again** fetches a bounded HTTPS release manifest. HomeBot accepts only a newer semantic version for the current platform and architecture whose protocol range includes this client and whose signing classification matches the platform. Artifact names cannot contain path separators.

Pressing **Download verified update** is a second explicit action. The download is size bounded, streamed to a unique temporary file, and renamed into the platform cache only after its exact byte count and SHA-256 match the manifest. A staged package is not silently executed. Installation remains an explicit platform package action, and server/client protocol negotiation still runs after restart.

## Migration contract

On every existing database whose recorded SQLx schema is older than the current `SCHEMA_VERSION`, HomeBot performs these operations in order:

1. open the source without running migrations;
2. run SQLite `quick_check`;
3. refuse a schema newer than the binary supports;
4. create `homebot.db.pre-migration-v<old>-to-v<new>.db` using SQLite `VACUUM INTO`;
5. restrict the backup to mode `0600` on Unix;
6. reopen the backup read-only, run `quick_check`, and verify its schema matches the source;
7. only then run the transactional SQLx migrations and final integrity/foreign-key checks.

If backup creation or verification fails—including an unwritable/full destination—the migration does not begin. A valid existing backup is reverified and reused after an interrupted launch. Corrupt databases fail closed. A binary never attempts to downgrade a newer schema.

## Recovery procedure

Stop every HomeBot desktop/server process before recovery. Never overwrite the only copy of a database.

macOS data defaults to `~/Library/Application Support/HomeBot/homebot.db`; Linux defaults to `${XDG_DATA_HOME:-$HOME/.local/share}/homebot/homebot.db`.

1. Identify the exact pre-migration backup beside the database.
2. Copy, do not move, the current database plus any `-wal` and `-shm` files into a separate incident directory.
3. Copy the selected pre-migration backup to a new path such as `homebot.recovered.db` with mode `0600`.
4. If `sqlite3` is available, run `sqlite3 homebot.recovered.db 'PRAGMA quick_check; PRAGMA foreign_key_check;'`. Expected output begins with `ok` and contains no foreign-key rows.
5. Start the same or newer HomeBot server temporarily with `HOMEBOT_DATABASE` pointing at the recovered copy and the normal credential source. Require a healthy endpoint, current migrations, expected Bots/chats, and preserved event history.
6. Only after those postconditions pass should an operator make the recovered copy the active database. Retain the incident copy and migration backup until the recovery is independently confirmed.

For a too-new schema, install a HomeBot binary that supports that schema; do not edit `_sqlx_migrations`. For corruption, preserve all files before diagnosis. For disk-full errors, free space on the same volume and retry so the existing verified backup can be reused.
