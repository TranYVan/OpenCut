// Test doubles for the classic export script: just enough of IndexedDB and
// OPFS for what the script touches, plus a small zip reader to check output.

export interface ExportResult {
	id: string;
	name: string;
	ok: boolean;
	fileName?: string;
	mediaFiles?: number;
	missingFiles?: number;
	warnings?: string[];
	error?: string;
}

export interface ZipWriterLike {
	add(
		name: string,
		data: string | Uint8Array | Blob,
		options?: { modified?: Date },
	): Promise<string>;
	finish(): Blob;
}

export interface ClassicExportApi {
	run(options?: Record<string, unknown>): Promise<ExportResult[]>;
	listProjects(
		options?: Record<string, unknown>,
	): Promise<Array<{ id: string; name: string; version: number }>>;
	crc32(bytes: Uint8Array, crc?: number): number;
	ZipWriter: new (options?: { forceZip64?: boolean }) => ZipWriterLike;
}

/** databases: name -> store name -> [key, value] pairs */
export type FakeDatabases = Record<string, Record<string, Array<[unknown, unknown]>>>;

interface FakeRequest<T> {
	result?: T;
	error?: DOMException;
	transaction?: { abort(): void };
	onsuccess?: () => void;
	onerror?: (event: { preventDefault(): void }) => void;
	onupgradeneeded?: () => void;
	onblocked?: () => void;
}

function settle<T>(request: FakeRequest<T>, compute: () => T): FakeRequest<T> {
	setTimeout(() => {
		request.result = compute();
		request.onsuccess?.();
	}, 0);
	return request;
}

export function createFakeIndexedDB(databases: FakeDatabases) {
	const data = structuredClone(databases);

	const openDatabase = (name: string) => {
		const stores = data[name] ?? {};
		return {
			objectStoreNames: { contains: (store: string) => store in stores },
			close: () => undefined,
			transaction: (storeName: string, mode: string) => {
				if (mode !== "readonly") {
					throw new Error(`unexpected ${mode} transaction on ${name}`);
				}
				const rows = stores[storeName] ?? [];
				return {
					objectStore: () => ({
						getAllKeys: () => settle({}, () => rows.map(([key]) => key)),
						getAll: () => settle({}, () => rows.map(([, value]) => structuredClone(value))),
					}),
				};
			},
		};
	};

	return {
		open(name: string, version?: number) {
			if (version !== undefined) {
				throw new Error("the export must open databases at their current version");
			}
			const request: FakeRequest<ReturnType<typeof openDatabase>> = {};
			setTimeout(() => {
				if (!(name in data)) {
					let aborted = false;
					request.transaction = {
						abort: () => {
							aborted = true;
						},
					};
					request.onupgradeneeded?.();
					if (aborted) {
						request.error = new DOMException("Version change aborted", "AbortError");
						request.onerror?.({ preventDefault: () => undefined });
						return;
					}
					data[name] = {};
				}
				request.result = openDatabase(name);
				request.onsuccess?.();
			}, 0);
			return request;
		},
		databaseNames: () => Object.keys(data).sort(),
	};
}

/** directories: OPFS directory name -> file name -> File */
export function createFakeStorage(directories: Record<string, Record<string, File>>) {
	const directoryHandle = (files: Record<string, File>) => ({
		kind: "directory",
		async *entries() {
			for (const [name, file] of Object.entries(files)) {
				yield [name, { kind: "file", getFile: async () => file }] as const;
			}
		},
	});

	return {
		getDirectory: async () => ({
			getDirectoryHandle: async (name: string, options?: { create?: boolean }) => {
				if (options?.create) {
					throw new Error("the export must not create OPFS directories");
				}
				const files = directories[name];
				if (!files) {
					throw new DOMException(`${name} not found`, "NotFoundError");
				}
				return directoryHandle(files);
			},
		}),
	};
}

export interface ZipEntry {
	name: string;
	data: Uint8Array;
	zip64: boolean;
}

/**
 * Minimal zip reader for stored (uncompressed) archives, ZIP64 included.
 * Walks the central directory, checks every local header and CRC.
 */
export async function readZip(
	blob: Blob,
	crc32: (bytes: Uint8Array) => number,
): Promise<ZipEntry[]> {
	const bytes = new Uint8Array(await blob.arrayBuffer());
	const view = new DataView(bytes.buffer);
	const decoder = new TextDecoder();

	const endAt = bytes.length - 22;
	if (view.getUint32(endAt, true) !== 0x06054b50) {
		throw new Error("missing end of central directory record");
	}
	let count = view.getUint16(endAt + 10, true);
	let centralOffset = view.getUint32(endAt + 16, true);
	if (centralOffset === 0xffffffff) {
		const locatorAt = endAt - 20;
		if (view.getUint32(locatorAt, true) !== 0x07064b50) {
			throw new Error("missing ZIP64 locator");
		}
		const end64At = Number(view.getBigUint64(locatorAt + 8, true));
		if (view.getUint32(end64At, true) !== 0x06064b50) {
			throw new Error("missing ZIP64 end of central directory record");
		}
		count = Number(view.getBigUint64(end64At + 32, true));
		centralOffset = Number(view.getBigUint64(end64At + 48, true));
	}

	const entries: ZipEntry[] = [];
	let at = centralOffset;
	for (let i = 0; i < count; i += 1) {
		if (view.getUint32(at, true) !== 0x02014b50) {
			throw new Error(`bad central directory header ${i}`);
		}
		if (view.getUint16(at + 10, true) !== 0) {
			throw new Error("expected STORE method");
		}
		const crc = view.getUint32(at + 16, true);
		let size = view.getUint32(at + 24, true);
		const nameLength = view.getUint16(at + 28, true);
		const extraLength = view.getUint16(at + 30, true);
		const commentLength = view.getUint16(at + 32, true);
		let localOffset = view.getUint32(at + 42, true);
		const name = decoder.decode(bytes.subarray(at + 46, at + 46 + nameLength));

		let zip64 = false;
		let extraAt = at + 46 + nameLength;
		const extraEnd = extraAt + extraLength;
		while (extraAt < extraEnd) {
			const id = view.getUint16(extraAt, true);
			const length = view.getUint16(extraAt + 2, true);
			if (id === 0x0001) {
				zip64 = true;
				let field = extraAt + 4;
				if (size === 0xffffffff) {
					size = Number(view.getBigUint64(field, true));
					field += 16; // uncompressed + compressed
				}
				if (localOffset === 0xffffffff) {
					localOffset = Number(view.getBigUint64(field, true));
				}
			}
			extraAt += 4 + length;
		}

		if (view.getUint32(localOffset, true) !== 0x04034b50) {
			throw new Error(`bad local header for ${name}`);
		}
		const dataAt =
			localOffset +
			30 +
			view.getUint16(localOffset + 26, true) +
			view.getUint16(localOffset + 28, true);
		const data = bytes.slice(dataAt, dataAt + size);
		if (crc32(data) !== crc) {
			throw new Error(`CRC mismatch for ${name}`);
		}
		entries.push({ name, data, zip64 });
		at += 46 + nameLength + extraLength + commentLength;
	}
	return entries;
}
