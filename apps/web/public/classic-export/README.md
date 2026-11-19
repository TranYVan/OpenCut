# Export your projects from classic OpenCut

The classic OpenCut web editor (opencut.app) keeps your projects inside your
browser, in IndexedDB (project data) and the Origin Private File System
(media files). The new OpenCut can't read the browser's private storage, so
this script copies everything out: **one `.zip` per project**.

The script only reads. It doesn't change, upload or delete anything.

## Steps

1. Use the **same browser and profile** you edited your projects in
   (projects are stored per browser, per profile).
2. Open **https://opencut.app** (the site you used, e.g. the projects page).
   Close other opencut.app tabs.
3. Open the developer console:
   - Chrome / Edge / Brave: `Ctrl+Shift+J` (Windows/Linux) or `Cmd+Option+J` (macOS)
   - Firefox: `Ctrl+Shift+K` or `Cmd+Option+K`
   - Safari: enable *Settings → Advanced → Show features for web developers*,
     then `Cmd+Option+C`
4. Open [`opencut-classic-export.js`](./opencut-classic-export.js), select all
   of it and copy it.
5. Paste it into the console and press Enter. If the browser refuses the
   paste, type `allow pasting` (Chrome) and press Enter, then paste again.
6. The console shows progress. Your browser downloads the zips one after
   another. If it asks whether opencut.app may **download multiple files**,
   choose *Allow*.
7. When it prints `Done`, a table lists each project, its zip and any
   missing media. Keep the zips somewhere safe.

Large projects take a while: every media file is read once to compute its
checksum before the download starts.

## What's in a zip

| Path                     | Contents                                                         |
| ------------------------ | ---------------------------------------------------------------- |
| `project.json`           | The project exactly as classic OpenCut stored it (any version)   |
| `media.json`             | Stored info for each media item, and where its file is in the zip |
| `media/<id>/<file name>` | Your original media files, unmodified                            |
| `legacy/*.json`          | Timelines of very old projects never reopened since (rare)       |
| `export-info.json`       | Export date, counts and any warnings                             |

Each item in `media.json` has a `status`:

- `ok`: the file is in the zip.
- `missing-file`: the browser no longer has the file (for example after
  clearing site data). Re-import it from your own copy.
- `unreadable-file`: the file exists but the browser failed to read it.
- `missing-metadata`: a stored file with no media entry, kept just in case.

## Useful commands

After the first run, these work in the same console:

```js
await opencutClassicExport.listProjects()            // ids, names, versions
await opencutClassicExport.run({ only: ["<id>"] })   // export chosen projects again
```

## Troubleshooting

- **"No classic projects found"**: wrong site, browser or profile, or the
  browser's site data was cleared. Projects made at another address (e.g. a
  preview or self-hosted URL) must be exported from that address.
- **"blocked by another tab"**: close other opencut.app tabs and run it again.
- **A download fails or is blocked**: allow multiple downloads for
  opencut.app, then rerun just that project with
  `opencutClassicExport.run({ only: ["<id>"] })`.
- **Not enough disk space**: each zip is about as large as its media, and the
  browser may need space for a temporary copy while downloading.

## Limits

- Files are stored uncompressed (media is already compressed). Zips over
  4 GB, or with files over 4 GB, use ZIP64. 7-Zip, `unzip` and macOS
  Archive Utility open them; if your system's built-in tool can't, use 7-Zip.
- Projects with huge media rely on the browser handling large downloads.
  Chromium browsers do; if one fails in another browser, try Chrome or Edge.
- Saved sound effects (a separate library, not part of any project) aren't
  exported.
