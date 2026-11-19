// @vitest-environment node
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
	type ClassicExportApi,
	createFakeIndexedDB,
	createFakeStorage,
	readZip,
} from "./fakes";

const SCRIPT_PATH = new URL(
	"../../public/classic-export/opencut-classic-export.js",
	import.meta.url,
);

function loadScript(): ClassicExportApi {
	const source = readFileSync(SCRIPT_PATH, "utf8");
	let api: ClassicExportApi | undefined;
	const hookHost = globalThis as { __opencutClassicExportTestHook?: unknown };
	hookHost.__opencutClassicExportTestHook = (received: ClassicExportApi) => {
		api = received;
	};
	try {
		new Function(source)();
	} finally {
		hookHost.__opencutClassicExportTestHook = undefined;
	}
	if (!api) {
		throw new Error("script did not hand over its API");
	}
	return api;
}

const hasUnzip = spawnSync("unzip", ["-v"]).status === 0;

const silentLog = {
	log: () => undefined,
	warn: () => undefined,
	error: () => undefined,
};

const encoder = new TextEncoder();
const decoder = new TextDecoder();

let api: ClassicExportApi;
let workDir: string;

beforeAll(() => {
	api = loadScript();
	workDir = mkdtempSync(join(tmpdir(), "opencut-classic-export-"));
});

afterAll(() => {
	rmSync(workDir, { recursive: true, force: true });
});

async function expectValidZip(blob: Blob, name: string): Promise<void> {
	if (!hasUnzip) {
		return;
	}
	const path = join(workDir, name);
	writeFileSync(path, new Uint8Array(await blob.arrayBuffer()));
	const output = execFileSync("unzip", ["-t", path], { encoding: "utf8" });
	expect(output).toContain("No errors detected");
}

describe("crc32", () => {
	it("matches the standard check value", () => {
		expect(api.crc32(encoder.encode("123456789"))).toBe(0xcbf43926);
		expect(api.crc32(new Uint8Array())).toBe(0);
	});
});

describe("ZipWriter", () => {
	const contents = {
		"hello.txt": "hello world\n",
		"nested/dir/ünïcödé ✂.json": JSON.stringify({ a: 1 }),
		"empty.bin": "",
	};

	for (const forceZip64 of [false, true]) {
		it(`writes a valid archive${forceZip64 ? " with ZIP64 records" : ""}`, async () => {
			const zip = new api.ZipWriter({ forceZip64 });
			for (const [name, text] of Object.entries(contents)) {
				await zip.add(name, text, { modified: new Date(2024, 4, 6, 7, 8, 10) });
			}
			const bytes = new Uint8Array(1024 * 1024 + 7).map((_, i) => (i * 31) % 251);
			await zip.add("big/data.bin", new Blob([bytes]));
			// Duplicate names get a suffix instead of overwriting.
			expect(await zip.add("hello.txt", "second")).toBe("hello (2).txt");

			const blob = zip.finish();
			const entries = await readZip(blob, api.crc32);

			expect(entries.map((entry) => entry.name)).toEqual([
				...Object.keys(contents),
				"big/data.bin",
				"hello (2).txt",
			]);
			for (const [name, text] of Object.entries(contents)) {
				expect(decoder.decode(entries.find((e) => e.name === name)?.data)).toBe(text);
			}
			expect(entries.find((e) => e.name === "big/data.bin")?.data).toEqual(bytes);
			expect(entries.every((entry) => entry.zip64 === forceZip64)).toBe(true);

			await expectValidZip(blob, `writer-${forceZip64 ? "zip64" : "plain"}.zip`);
		});
	}
});

describe("project export", () => {
	const videoBytes = new Uint8Array(300_000).map((_, i) => i % 256);

	function classicBrowser() {
		const indexedDB = createFakeIndexedDB({
			"video-editor-projects": {
				projects: [
					[
						"p-current",
						{
							id: "p-current",
							version: 31,
							metadata: {
								id: "p-current",
								name: "Holiday / Edit",
								createdAt: "2025-01-01T00:00:00.000Z",
								updatedAt: "2025-02-01T00:00:00.000Z",
							},
							scenes: [{ id: "s1", name: "Main", isMain: true, tracks: {} }],
							custom: { nested: [1, 2, 3] },
						},
					],
					[
						"p-legacy",
						{
							id: "p-legacy",
							metadata: { id: "p-legacy", name: "Old one" },
							scenes: [{ id: "s9", isMain: true }],
						},
					],
					["broken", "not a project"],
				],
			},
			"video-editor-media-p-current": {
				"media-metadata": [
					["m1", { id: "m1", name: "clip.mp4", type: "video", size: videoBytes.length }],
					["m2", { id: "m2", name: "gone.png", type: "image", size: 10 }],
				],
			},
			"video-editor-timelines-p-legacy-s9": {
				timeline: [["timeline", { tracks: [{ id: "t1" }], lastModified: "x" }]],
			},
		});
		const storage = createFakeStorage({
			"media-files-p-current": {
				m1: new File([videoBytes], "m1", { type: "video/mp4", lastModified: 1_700_000_000_000 }),
				orphan: new File(["orphan data"], "orphan"),
			},
		});
		return { indexedDB, storage };
	}

	it("lists projects of any storage version and skips malformed entries", async () => {
		const { indexedDB, storage } = classicBrowser();
		const projects = await api.listProjects({ indexedDB, storage, log: silentLog });
		expect(projects).toEqual([
			{ id: "p-current", name: "Holiday / Edit", version: 31 },
			{ id: "p-legacy", name: "Old one", version: 1 },
		]);
	});

	it("downloads one zip per project with raw JSON, media and metadata", async () => {
		const { indexedDB, storage } = classicBrowser();
		const downloads: Array<{ blob: Blob; fileName: string }> = [];

		const results = await api.run({
			indexedDB,
			storage,
			log: silentLog,
			origin: "https://opencut.app",
			delayBetweenDownloadsMs: 0,
			download: (blob: Blob, fileName: string) => {
				downloads.push({ blob, fileName });
			},
		});

		expect(results.map((result) => result.ok)).toEqual([true, true]);
		expect(downloads.map((download) => download.fileName)).toEqual([
			"opencut-Holiday _ Edit-p-curren.zip",
			"opencut-Old one-p-legacy.zip",
		]);

		const current = await readZip(downloads[0].blob, api.crc32);
		const byName = new Map(current.map((entry) => [entry.name, entry.data]));
		expect([...byName.keys()]).toEqual([
			"project.json",
			"media/m1/clip.mp4",
			"media/orphan/orphan",
			"media.json",
			"export-info.json",
		]);

		const project = JSON.parse(decoder.decode(byName.get("project.json")));
		expect(project.custom).toEqual({ nested: [1, 2, 3] });
		expect(project.version).toBe(31);
		expect(byName.get("media/m1/clip.mp4")).toEqual(videoBytes);

		const media = JSON.parse(decoder.decode(byName.get("media.json")));
		expect(
			media.items.map((item: { id: string; status: string }) => [item.id, item.status]),
		).toEqual([
			["m1", "ok"],
			["m2", "missing-file"],
			["orphan", "missing-metadata"],
		]);
		expect(media.items[0].file).toMatchObject({
			path: "media/m1/clip.mp4",
			size: videoBytes.length,
			mimeType: "video/mp4",
		});

		const info = JSON.parse(decoder.decode(byName.get("export-info.json")));
		expect(info.source).toBe("https://opencut.app");
		expect(info.counts).toMatchObject({ mediaItems: 3, mediaFiles: 2, missingFiles: 1 });
		expect(info.warnings).toHaveLength(2);

		const legacy = await readZip(downloads[1].blob, api.crc32);
		const legacyNames = legacy.map((entry) => entry.name);
		expect(legacyNames).toContain("legacy/video-editor-timelines-p-legacy-s9.json");
		const timeline = JSON.parse(
			decoder.decode(
				legacy.find((entry) => entry.name.startsWith("legacy/"))?.data,
			),
		);
		expect(timeline.entries[0].value.tracks).toEqual([{ id: "t1" }]);

		await expectValidZip(downloads[0].blob, "project-current.zip");
		await expectValidZip(downloads[1].blob, "project-legacy.zip");
	});

	it("never creates databases while probing for them", async () => {
		const { indexedDB, storage } = classicBrowser();
		const before = indexedDB.databaseNames();
		await api.run({
			indexedDB,
			storage,
			log: silentLog,
			delayBetweenDownloadsMs: 0,
			download: () => undefined,
		});
		expect(indexedDB.databaseNames()).toEqual(before);
	});

	it("exports projects without media when OPFS is unavailable", async () => {
		const { indexedDB } = classicBrowser();
		const downloads: Blob[] = [];
		const results = await api.run({
			indexedDB,
			storage: undefined,
			log: silentLog,
			only: ["p-current"],
			delayBetweenDownloadsMs: 0,
			download: (blob: Blob) => {
				downloads.push(blob);
			},
		});
		expect(results).toHaveLength(1);
		expect(results[0].ok).toBe(true);
		expect(results[0].missingFiles).toBe(2);
		const entries = await readZip(downloads[0], api.crc32);
		expect(entries.map((entry) => entry.name)).toEqual([
			"project.json",
			"media.json",
			"export-info.json",
		]);
	});

	it("reports no projects when the classic database does not exist", async () => {
		const indexedDB = createFakeIndexedDB({});
		const results = await api.run({
			indexedDB,
			storage: createFakeStorage({}),
			log: silentLog,
			download: () => undefined,
		});
		expect(results).toEqual([]);
		expect(indexedDB.databaseNames()).toEqual([]);
	});
});
