/*
 * OpenCut classic project export
 * ------------------------------
 * Paste this whole file into the browser devtools console while you are on
 * https://opencut.app (the classic web editor). It reads every project the
 * classic editor saved in this browser and downloads one .zip per project.
 *
 * Nothing is uploaded anywhere and nothing is changed or deleted: the script
 * only reads IndexedDB and the Origin Private File System (OPFS) and hands
 * the zips to your browser's normal download mechanism.
 *
 * Each zip contains:
 *   project.json            the stored project record, exactly as saved
 *                           (any storage version, not migrated)
 *   media.json              the stored metadata of every media item, plus
 *                           where its file is inside the zip (or why not)
 *   media/<id>/<file name>  the original media files
 *   legacy/<db name>.json   timelines of very old projects (only when present)
 *   export-info.json        exporter version, date, counts and warnings
 *
 * After it has run, `opencutClassicExport` stays available in the console:
 *   await opencutClassicExport.listProjects()           // ids and names
 *   await opencutClassicExport.run({ only: ["<id>"] })  // export some again
 *
 * See README.md next to this file for the step-by-step guide.
 */
(() => {
	"use strict";

	const EXPORTER = "opencut-classic-export";
	const EXPORTER_VERSION = 1;
	const LOG_PREFIX = "[OpenCut export]";

	// Storage layout of the classic editor (apps/web/src/services/storage at
	// tag `pre-rewrite`). These names never changed across storage versions.
	const PROJECTS_DB = "video-editor-projects";
	const PROJECTS_STORE = "projects";
	const MEDIA_DB_PREFIX = "video-editor-media-"; // + projectId
	const MEDIA_STORE = "media-metadata";
	const MEDIA_DIR_PREFIX = "media-files-"; // OPFS directory + projectId
	// Projects saved before storage v2 kept their timelines in separate
	// databases, which the v1 -> v2 migration folded into the project and then
	// deleted. They only still exist if the project was never opened again.
	const LEGACY_TIMELINE_DB_PREFIX = "video-editor-timelines-"; // + projectId[-sceneId]
	const LEGACY_TIMELINE_STORE = "timeline";

	// ------------------------------------------------------------------------
	// Small helpers
	// ------------------------------------------------------------------------

	const isRecord = (value) => typeof value === "object" && value !== null;

	function formatBytes(bytes) {
		const units = ["B", "KB", "MB", "GB", "TB"];
		let value = bytes;
		let unit = 0;
		while (value >= 1024 && unit < units.length - 1) {
			value /= 1024;
			unit += 1;
		}
		return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
	}

	/** Make a string safe to use as one path segment inside a zip. */
	function safePathSegment(name, fallback) {
		const cleaned = String(name ?? "")
			// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping control characters is the point
			.replace(/[\u0000-\u001f\u007f<>:"/\\|?*]/g, "_")
			.replace(/^[\s.]+|[\s.]+$/g, "")
			.slice(0, 120);
		return cleaned.length > 0 ? cleaned : fallback;
	}

	/**
	 * JSON.stringify replacer for values IndexedDB can hold but JSON can't.
	 * Plain project data (strings, numbers, arrays, objects) passes through
	 * unchanged; Dates already serialize as ISO strings.
	 */
	function jsonReplacer(_key, value) {
		if (typeof value === "bigint") {
			return { $type: "bigint", value: value.toString() };
		}
		if (value instanceof Map) {
			return { $type: "map", entries: [...value.entries()] };
		}
		if (value instanceof Set) {
			return { $type: "set", values: [...value.values()] };
		}
		if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
			const bytes =
				value instanceof ArrayBuffer
					? new Uint8Array(value)
					: new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
			let binary = "";
			for (const byte of bytes) {
				binary += String.fromCharCode(byte);
			}
			return { $type: "bytes", base64: btoa(binary) };
		}
		if (typeof Blob !== "undefined" && value instanceof Blob) {
			return {
				$type: "blob",
				size: value.size,
				mimeType: value.type,
				note: "binary value not exported",
			};
		}
		return value;
	}

	const toJson = (value) => JSON.stringify(value, jsonReplacer, 2);

	// ------------------------------------------------------------------------
	// CRC-32 (the checksum zip uses)
	// ------------------------------------------------------------------------

	const CRC_TABLE = (() => {
		const table = new Uint32Array(256);
		for (let n = 0; n < 256; n += 1) {
			let c = n;
			for (let k = 0; k < 8; k += 1) {
				c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
			}
			table[n] = c >>> 0;
		}
		return table;
	})();

	/** Continue a CRC-32 over `bytes`. Start with crc = 0. */
	function crc32(bytes, crc = 0) {
		let c = crc ^ 0xffffffff;
		for (let i = 0; i < bytes.length; i += 1) {
			c = CRC_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
		}
		return (c ^ 0xffffffff) >>> 0;
	}

	/** CRC-32 of a Blob/File, read in chunks so large files never sit in memory. */
	async function crc32OfBlob(blob, onProgress) {
		const reader = blob.stream().getReader();
		let crc = 0;
		let done = 0;
		for (;;) {
			const chunk = await reader.read();
			if (chunk.done) {
				break;
			}
			crc = crc32(chunk.value, crc);
			done += chunk.value.length;
			onProgress?.(done, blob.size);
		}
		return crc;
	}

	// ------------------------------------------------------------------------
	// Zip writer: STORE (no compression), UTF-8 names, ZIP64 when needed.
	//
	// Media is already compressed, so storing is fast and loses nothing. File
	// contents are not copied: the resulting Blob references the original
	// File objects, so multi-gigabyte projects don't need that much RAM.
	// ------------------------------------------------------------------------

	const UINT16_MAX = 0xffff;
	const UINT32_MAX = 0xffffffff;
	const FLAG_UTF8 = 0x0800;
	const VERSION_DEFAULT = 20; // 2.0: plain stored files
	const VERSION_ZIP64 = 45; // 4.5: ZIP64 extensions

	function dosDateTime(date) {
		const d = Number.isNaN(date.getTime()) ? new Date() : date;
		const year = Math.min(Math.max(d.getFullYear(), 1980), 2107);
		return {
			time: (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1),
			date: ((year - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate(),
		};
	}

	class ZipWriter {
		/**
		 * @param {{ forceZip64?: boolean }} [options] forceZip64 writes ZIP64
		 *   records even for small archives (used by the tests).
		 */
		constructor(options = {}) {
			this.forceZip64 = options.forceZip64 === true;
			this.parts = [];
			this.entries = [];
			this.offset = 0;
			this.names = new Set();
		}

		/** Return `name`, or `name` with a numeric suffix if already used. */
		uniqueName(name) {
			if (!this.names.has(name)) {
				this.names.add(name);
				return name;
			}
			const dot = name.lastIndexOf(".");
			const slash = name.lastIndexOf("/");
			const hasExt = dot > slash + 1;
			const base = hasExt ? name.slice(0, dot) : name;
			const ext = hasExt ? name.slice(dot) : "";
			let n = 2;
			while (this.names.has(`${base} (${n})${ext}`)) {
				n += 1;
			}
			const unique = `${base} (${n})${ext}`;
			this.names.add(unique);
			return unique;
		}

		/**
		 * Add a file. `data` is a string, Uint8Array or Blob/File.
		 * Returns the name actually used inside the zip.
		 */
		async add(name, data, options = {}) {
			const blob =
				typeof data === "string" || data instanceof Uint8Array
					? new Blob([data])
					: data;
			const size = blob.size;
			const crc =
				typeof data === "string"
					? crc32(new TextEncoder().encode(data))
					: data instanceof Uint8Array
						? crc32(data)
						: await crc32OfBlob(blob, options.onProgress);

			const finalName = this.uniqueName(name);
			const nameBytes = new TextEncoder().encode(finalName);
			const { time, date } = dosDateTime(options.modified ?? new Date());
			const zip64 = this.forceZip64 || size >= UINT32_MAX;

			// Local file header (+ ZIP64 extra with both sizes when needed).
			const extraLength = zip64 ? 20 : 0;
			const header = new DataView(
				new ArrayBuffer(30 + nameBytes.length + extraLength),
			);
			header.setUint32(0, 0x04034b50, true);
			header.setUint16(4, zip64 ? VERSION_ZIP64 : VERSION_DEFAULT, true);
			header.setUint16(6, FLAG_UTF8, true);
			header.setUint16(8, 0, true); // method: store
			header.setUint16(10, time, true);
			header.setUint16(12, date, true);
			header.setUint32(14, crc, true);
			header.setUint32(18, zip64 ? UINT32_MAX : size, true);
			header.setUint32(22, zip64 ? UINT32_MAX : size, true);
			header.setUint16(26, nameBytes.length, true);
			header.setUint16(28, extraLength, true);
			new Uint8Array(header.buffer).set(nameBytes, 30);
			if (zip64) {
				const extra = 30 + nameBytes.length;
				header.setUint16(extra, 0x0001, true);
				header.setUint16(extra + 2, 16, true);
				header.setBigUint64(extra + 4, BigInt(size), true);
				header.setBigUint64(extra + 12, BigInt(size), true);
			}

			this.entries.push({
				nameBytes,
				crc,
				size,
				time,
				date,
				zip64,
				offset: this.offset,
			});
			this.parts.push(header.buffer, blob);
			this.offset += header.byteLength + size;
			return finalName;
		}

		/** Total bytes written so far (before the central directory). */
		get byteLength() {
			return this.offset;
		}

		/** Write the central directory and return the finished zip Blob. */
		finish() {
			const centralStart = this.offset;
			let centralSize = 0;

			for (const entry of this.entries) {
				const bigSize = entry.zip64;
				const bigOffset = this.forceZip64 || entry.offset >= UINT32_MAX;
				const extraLength =
					bigSize || bigOffset ? 4 + (bigSize ? 16 : 0) + (bigOffset ? 8 : 0) : 0;
				const needsZip64 = extraLength > 0;
				const record = new DataView(
					new ArrayBuffer(46 + entry.nameBytes.length + extraLength),
				);
				record.setUint32(0, 0x02014b50, true);
				record.setUint16(4, VERSION_ZIP64, true); // made by
				record.setUint16(6, needsZip64 ? VERSION_ZIP64 : VERSION_DEFAULT, true);
				record.setUint16(8, FLAG_UTF8, true);
				record.setUint16(10, 0, true);
				record.setUint16(12, entry.time, true);
				record.setUint16(14, entry.date, true);
				record.setUint32(16, entry.crc, true);
				record.setUint32(20, bigSize ? UINT32_MAX : entry.size, true);
				record.setUint32(24, bigSize ? UINT32_MAX : entry.size, true);
				record.setUint16(28, entry.nameBytes.length, true);
				record.setUint16(30, extraLength, true);
				record.setUint16(32, 0, true); // comment length
				record.setUint16(34, 0, true); // disk number
				record.setUint16(36, 0, true); // internal attributes
				record.setUint32(38, 0, true); // external attributes
				record.setUint32(42, bigOffset ? UINT32_MAX : entry.offset, true);
				new Uint8Array(record.buffer).set(entry.nameBytes, 46);
				if (needsZip64) {
					let at = 46 + entry.nameBytes.length;
					record.setUint16(at, 0x0001, true);
					record.setUint16(at + 2, extraLength - 4, true);
					at += 4;
					if (bigSize) {
						record.setBigUint64(at, BigInt(entry.size), true);
						record.setBigUint64(at + 8, BigInt(entry.size), true);
						at += 16;
					}
					if (bigOffset) {
						record.setBigUint64(at, BigInt(entry.offset), true);
					}
				}
				this.parts.push(record.buffer);
				centralSize += record.byteLength;
			}

			const count = this.entries.length;
			const needsZip64End =
				this.forceZip64 ||
				count >= UINT16_MAX ||
				centralStart >= UINT32_MAX ||
				centralSize >= UINT32_MAX;

			if (needsZip64End) {
				const zip64EndOffset = centralStart + centralSize;
				const end64 = new DataView(new ArrayBuffer(56 + 20));
				end64.setUint32(0, 0x06064b50, true);
				end64.setBigUint64(4, 44n, true); // size of the rest of this record
				end64.setUint16(12, VERSION_ZIP64, true);
				end64.setUint16(14, VERSION_ZIP64, true);
				end64.setUint32(16, 0, true); // this disk
				end64.setUint32(20, 0, true); // disk with central directory
				end64.setBigUint64(24, BigInt(count), true);
				end64.setBigUint64(32, BigInt(count), true);
				end64.setBigUint64(40, BigInt(centralSize), true);
				end64.setBigUint64(48, BigInt(centralStart), true);
				// ZIP64 end of central directory locator
				end64.setUint32(56, 0x07064b50, true);
				end64.setUint32(60, 0, true);
				end64.setBigUint64(64, BigInt(zip64EndOffset), true);
				end64.setUint32(72, 1, true); // total disks
				this.parts.push(end64.buffer);
			}

			const end = new DataView(new ArrayBuffer(22));
			end.setUint32(0, 0x06054b50, true);
			end.setUint16(4, 0, true);
			end.setUint16(6, 0, true);
			end.setUint16(8, needsZip64End ? UINT16_MAX : count, true);
			end.setUint16(10, needsZip64End ? UINT16_MAX : count, true);
			end.setUint32(12, needsZip64End ? UINT32_MAX : centralSize, true);
			end.setUint32(16, needsZip64End ? UINT32_MAX : centralStart, true);
			end.setUint16(20, 0, true); // comment length
			this.parts.push(end.buffer);

			return new Blob(this.parts, { type: "application/zip" });
		}
	}

	// ------------------------------------------------------------------------
	// Reading IndexedDB (read-only; never creates or upgrades a database)
	// ------------------------------------------------------------------------

	const promisify = (request) =>
		new Promise((resolve, reject) => {
			request.onsuccess = () => resolve(request.result);
			request.onerror = () => reject(request.error);
		});

	/**
	 * Open an existing database at its current version, or resolve null if it
	 * doesn't exist. Opening a missing database would create it, so the
	 * creation is aborted in `onupgradeneeded`.
	 */
	function openExistingDatabase(idb, name) {
		return new Promise((resolve, reject) => {
			let missing = false;
			const request = idb.open(name);
			request.onupgradeneeded = () => {
				missing = true;
				request.transaction.abort();
			};
			request.onsuccess = () => resolve(request.result);
			request.onerror = (event) => {
				if (missing) {
					event?.preventDefault?.();
					resolve(null);
				} else {
					reject(request.error);
				}
			};
			request.onblocked = () => {
				reject(
					new Error(
						`Database "${name}" is blocked by another tab. Close other OpenCut tabs and retry.`,
					),
				);
			};
		});
	}

	/** All [key, value] pairs of a store, or [] if the database/store is missing. */
	async function readStore(idb, dbName, storeName) {
		const db = await openExistingDatabase(idb, dbName);
		if (!db) {
			return [];
		}
		try {
			if (!db.objectStoreNames.contains(storeName)) {
				return [];
			}
			const store = db.transaction(storeName, "readonly").objectStore(storeName);
			const [keys, values] = await Promise.all([
				promisify(store.getAllKeys()),
				promisify(store.getAll()),
			]);
			return keys.map((key, index) => [key, values[index]]);
		} finally {
			db.close();
		}
	}

	// ------------------------------------------------------------------------
	// Reading OPFS
	// ------------------------------------------------------------------------

	/** Map of file name -> File in an OPFS directory; null if it doesn't exist. */
	async function readOpfsDirectory(storage, directoryName) {
		if (!storage || typeof storage.getDirectory !== "function") {
			return null;
		}
		const root = await storage.getDirectory();
		let directory;
		try {
			directory = await root.getDirectoryHandle(directoryName);
		} catch (error) {
			if (error?.name === "NotFoundError" || error?.name === "TypeMismatchError") {
				return null;
			}
			throw error;
		}
		const files = new Map();
		for await (const [name, handle] of directory.entries()) {
			if (handle.kind === "file") {
				files.set(name, await handle.getFile());
			}
		}
		return files;
	}

	// ------------------------------------------------------------------------
	// Classic project records
	// ------------------------------------------------------------------------

	/** Same rules as the classic migration runner's getProjectId. */
	function getProjectId(record, key) {
		if (typeof record.id === "string" && record.id.length > 0) {
			return record.id;
		}
		if (isRecord(record.metadata) && typeof record.metadata.id === "string") {
			return record.metadata.id;
		}
		return typeof key === "string" ? key : null;
	}

	function getProjectName(record) {
		if (isRecord(record.metadata) && typeof record.metadata.name === "string") {
			return record.metadata.name;
		}
		if (typeof record.name === "string") {
			return record.name; // storage v0
		}
		return null;
	}

	function getProjectVersion(record) {
		if (typeof record.version === "number") {
			return record.version;
		}
		return Array.isArray(record.scenes) && record.scenes.length > 0 ? 1 : 0;
	}

	/** Names of the pre-v2 timeline databases a project may still have. */
	function legacyTimelineDbNames(projectId, record) {
		const names = [`${LEGACY_TIMELINE_DB_PREFIX}${projectId}`];
		if (Array.isArray(record.scenes)) {
			for (const scene of record.scenes) {
				if (isRecord(scene) && typeof scene.id === "string") {
					names.push(`${LEGACY_TIMELINE_DB_PREFIX}${projectId}-${scene.id}`);
				}
			}
		}
		return names;
	}

	/** List the projects stored by the classic editor. */
	async function listProjects(env) {
		const entries = await readStore(env.indexedDB, PROJECTS_DB, PROJECTS_STORE);
		const projects = [];
		for (const [key, record] of entries) {
			if (!isRecord(record)) {
				env.log.warn(`${LOG_PREFIX} Skipping a malformed project entry`, key);
				continue;
			}
			const id = getProjectId(record, key);
			if (!id) {
				env.log.warn(`${LOG_PREFIX} Skipping a project without an id`, record);
				continue;
			}
			projects.push({
				id,
				name: getProjectName(record) ?? id,
				version: getProjectVersion(record),
				record,
			});
		}
		return projects;
	}

	// ------------------------------------------------------------------------
	// Exporting one project
	// ------------------------------------------------------------------------

	/** Build the zip for one project. Returns { blob, fileName, summary }. */
	async function buildProjectZip(env, project) {
		const { id, name, record } = project;
		const zip = new ZipWriter({ forceZip64: env.forceZip64 });
		const warnings = [];
		const warn = (message) => {
			warnings.push(message);
			env.log.warn(`${LOG_PREFIX}   ${message}`);
		};
		const modified = (() => {
			const raw = isRecord(record.metadata) ? record.metadata.updatedAt : record.updatedAt;
			const date = raw ? new Date(raw) : new Date();
			return Number.isNaN(date.getTime()) ? new Date() : date;
		})();

		await zip.add("project.json", toJson(record), { modified });

		// Media: metadata from IndexedDB, files from OPFS.
		const metadataEntries = await readStore(
			env.indexedDB,
			`${MEDIA_DB_PREFIX}${id}`,
			MEDIA_STORE,
		);
		let files = null;
		try {
			files = await readOpfsDirectory(env.storage, `${MEDIA_DIR_PREFIX}${id}`);
		} catch (error) {
			warn(`Could not read media files from OPFS: ${error?.message ?? error}`);
		}
		if (!env.storage || typeof env.storage.getDirectory !== "function") {
			warn("This browser has no OPFS access; media files can't be exported.");
		}
		files = files ?? new Map();

		const items = [];
		const usedFiles = new Set();
		let mediaBytes = 0;
		const total = metadataEntries.length;
		let index = 0;

		const addMediaFile = async (mediaId, displayName, file) => {
			const path = `media/${safePathSegment(mediaId, "unknown")}/${safePathSegment(displayName, safePathSegment(mediaId, "file"))}`;
			let lastPercent = -1;
			const finalPath = await zip.add(path, file, {
				modified: file.lastModified ? new Date(file.lastModified) : modified,
				onProgress: (done, size) => {
					if (size < 50 * 1024 * 1024) {
						return;
					}
					const percent = Math.floor((done / size) * 10) * 10;
					if (percent !== lastPercent) {
						lastPercent = percent;
						env.log.log(`${LOG_PREFIX}     reading ${displayName}: ${percent}%`);
					}
				},
			});
			mediaBytes += file.size;
			return {
				path: finalPath,
				size: file.size,
				mimeType: file.type,
				lastModified: file.lastModified,
			};
		};

		for (const [key, metadata] of metadataEntries) {
			index += 1;
			const mediaId =
				isRecord(metadata) && typeof metadata.id === "string" ? metadata.id : String(key);
			const displayName =
				isRecord(metadata) && typeof metadata.name === "string" ? metadata.name : mediaId;
			const file = files.get(mediaId);
			if (!file) {
				warn(`Media "${displayName}" (${mediaId}) has no stored file; skipped.`);
				items.push({ id: mediaId, status: "missing-file", metadata, file: null });
				continue;
			}
			usedFiles.add(mediaId);
			env.log.log(
				`${LOG_PREFIX}   media ${index}/${total}: ${displayName} (${formatBytes(file.size)})`,
			);
			try {
				const entry = await addMediaFile(mediaId, displayName, file);
				items.push({ id: mediaId, status: "ok", metadata, file: entry });
			} catch (error) {
				warn(`Could not read media "${displayName}" (${mediaId}): ${error?.message ?? error}`);
				items.push({ id: mediaId, status: "unreadable-file", metadata, file: null });
			}
		}

		// Files in OPFS without metadata: keep them too, they may still be used.
		for (const [fileName, file] of files) {
			if (usedFiles.has(fileName)) {
				continue;
			}
			warn(`File "${fileName}" has no media metadata; exported anyway.`);
			try {
				const entry = await addMediaFile(fileName, fileName, file);
				items.push({ id: fileName, status: "missing-metadata", metadata: null, file: entry });
			} catch (error) {
				warn(`Could not read file "${fileName}": ${error?.message ?? error}`);
			}
		}

		await zip.add(
			"media.json",
			toJson({
				projectId: id,
				source: {
					metadataDatabase: `${MEDIA_DB_PREFIX}${id}`,
					metadataStore: MEDIA_STORE,
					opfsDirectory: `${MEDIA_DIR_PREFIX}${id}`,
				},
				items,
			}),
			{ modified },
		);

		// Timelines of projects never migrated past storage v1.
		const legacy = [];
		for (const dbName of legacyTimelineDbNames(id, record)) {
			const entries = await readStore(env.indexedDB, dbName, LEGACY_TIMELINE_STORE);
			if (entries.length === 0) {
				continue;
			}
			const path = await zip.add(
				`legacy/${safePathSegment(dbName, "timeline")}.json`,
				toJson({
					database: dbName,
					store: LEGACY_TIMELINE_STORE,
					entries: entries.map(([key, value]) => ({ key, value })),
				}),
				{ modified },
			);
			legacy.push(path);
		}

		const counts = {
			mediaItems: items.length,
			mediaFiles: items.filter((item) => item.file !== null).length,
			missingFiles: items.filter((item) => item.file === null).length,
			mediaBytes,
		};

		await zip.add(
			"export-info.json",
			toJson({
				exporter: EXPORTER,
				exporterVersion: EXPORTER_VERSION,
				exportedAt: new Date().toISOString(),
				source: env.origin,
				project: { id, name, storageVersion: getProjectVersion(record) },
				counts,
				legacyTimelines: legacy,
				warnings,
			}),
		);

		const blob = zip.finish();
		const shortId = safePathSegment(id, "project").slice(0, 8);
		const fileName = `opencut-${safePathSegment(name, "project").slice(0, 60)}-${shortId}.zip`;
		return { blob, fileName, summary: { id, name, ...counts, warnings } };
	}

	// ------------------------------------------------------------------------
	// Downloading
	// ------------------------------------------------------------------------

	function browserDownload(blob, fileName) {
		const url = URL.createObjectURL(blob);
		const link = document.createElement("a");
		link.href = url;
		link.download = fileName;
		link.style.display = "none";
		document.body.appendChild(link);
		link.click();
		link.remove();
		// Keep the URL alive long enough for large downloads to start reading.
		setTimeout(() => URL.revokeObjectURL(url), 10 * 60 * 1000);
	}

	const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

	// ------------------------------------------------------------------------
	// Entry point
	// ------------------------------------------------------------------------

	function defaultEnv() {
		return {
			indexedDB: globalThis.indexedDB,
			storage: globalThis.navigator?.storage,
			origin: globalThis.location?.origin ?? null,
			download: browserDownload,
			log: console,
			delayBetweenDownloadsMs: 1500,
			forceZip64: false,
		};
	}

	/**
	 * Export projects one by one, downloading each zip before building the
	 * next. `options.only` limits the export to the given project ids.
	 */
	async function run(options = {}) {
		const env = { ...defaultEnv(), ...options };
		const { log } = env;
		if (!env.indexedDB) {
			throw new Error("IndexedDB is not available in this browser context.");
		}

		log.log(`${LOG_PREFIX} Reading projects from "${PROJECTS_DB}"...`);
		let projects = await listProjects(env);
		if (Array.isArray(env.only)) {
			projects = projects.filter((project) => env.only.includes(project.id));
		}
		if (projects.length === 0) {
			log.log(
				`${LOG_PREFIX} No classic projects found. Make sure you are on the site where you edited them (e.g. https://opencut.app) in the same browser profile.`,
			);
			return [];
		}
		log.log(`${LOG_PREFIX} Found ${projects.length} project(s).`);

		const results = [];
		let position = 0;
		for (const project of projects) {
			position += 1;
			log.log(
				`${LOG_PREFIX} [${position}/${projects.length}] "${project.name}" (${project.id}, storage v${project.version})`,
			);
			try {
				const { blob, fileName, summary } = await buildProjectZip(env, project);
				log.log(
					`${LOG_PREFIX}   downloading ${fileName} (${formatBytes(blob.size)}, ${summary.mediaFiles} media file(s))`,
				);
				await env.download(blob, fileName);
				results.push({ ...summary, fileName, ok: true });
				if (position < projects.length && env.delayBetweenDownloadsMs > 0) {
					await pause(env.delayBetweenDownloadsMs);
				}
			} catch (error) {
				log.error(`${LOG_PREFIX}   export of "${project.name}" failed:`, error);
				results.push({
					id: project.id,
					name: project.name,
					ok: false,
					error: String(error?.message ?? error),
				});
			}
		}

		const failed = results.filter((result) => !result.ok);
		const withWarnings = results.filter((result) => result.ok && result.warnings.length > 0);
		log.log(
			`${LOG_PREFIX} Done: ${results.length - failed.length} exported, ${failed.length} failed, ${withWarnings.length} with warnings.`,
		);
		if (failed.length > 0) {
			log.log(
				`${LOG_PREFIX} Retry failed ones with: await opencutClassicExport.run({ only: ${JSON.stringify(failed.map((result) => result.id))} })`,
			);
		}
		log.table?.(
			results.map((result) => ({
				name: result.name,
				id: result.id,
				ok: result.ok,
				mediaFiles: result.mediaFiles ?? 0,
				missingFiles: result.missingFiles ?? 0,
				file: result.fileName ?? result.error,
			})),
		);
		return results;
	}

	const api = {
		run,
		listProjects: async (options = {}) =>
			(await listProjects({ ...defaultEnv(), ...options })).map(({ id, name, version }) => ({
				id,
				name,
				version,
			})),
		// Building blocks, exposed for tests and for curious users.
		ZipWriter,
		crc32,
		buildProjectZip: (project, options = {}) =>
			buildProjectZip({ ...defaultEnv(), ...options }, project),
		listProjectRecords: (options = {}) => listProjects({ ...defaultEnv(), ...options }),
	};

	// Tests define this hook to receive the API without running the export.
	if (typeof globalThis.__opencutClassicExportTestHook === "function") {
		globalThis.__opencutClassicExportTestHook(api);
		return undefined;
	}

	globalThis.opencutClassicExport = api;
	return run();
})();
