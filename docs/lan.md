# LAN / Subsonic server

Melodie can serve its library to the local network as a read-only
OpenSubsonic API (PLAN.md §7, Tier 0). It is compiled in by default and
switched off at runtime.

## Enabling

In `~/.config/melodie/config.toml`:

```toml
lan_enabled = true
lan_port = 4533
lan_bind = ""            # empty = auto-detect this machine's LAN IP
lan_token = ""           # generated on first use; this is the password
lan_allow_public = false # refuse to bind a non-private address
```

Restart Melodie. It prints the address it bound.

## Pairing a phone

Click **Pair** in the window, or run `melodie --pair` for a headless box.
Both give you host, port and token; the button also shows a QR code that
the Melodie Android app scans.

For a third-party Subsonic client, enter:

- Server: `http://<host>:<port>`
- Username: anything (there are no accounts)
- Password: the `lan_token` value

## Supported methods

`ping`, `getLicense`, `getMusicFolders`, `getIndexes`, `getMusicDirectory`,
`getArtists`, `getArtist`, `getAlbum`, `getPlaylists`, `getPlaylist`,
`stream` (with `Range`), `download`, `getCoverArt`, `scrobble` (no-op).

Everything is read-only: there is no endpoint that writes to the library.

## Safety

- The token is required on every request; an unset token authenticates
  nothing.
- The server refuses to bind a public or wildcard address unless
  `lan_allow_public = true`.
- Responses are XML only.
