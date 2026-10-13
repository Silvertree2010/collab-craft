<?php

declare(strict_types=1);

namespace OCA\CollabCraft\Service;

/**
 * Which Craft app opens which file.
 *
 * The one source of truth for the mapping. The frontend receives this through
 * initial state rather than carrying its own copy, because two copies of a MIME
 * table drift and the symptom is an "Open in" entry that silently does nothing.
 *
 * None of these register as the default action. Collabora already owns the
 * default for the office formats through richdocuments, and taking it would
 * change what a double click does for every user on the instance. An
 * additional entry is the polite form, and it is also what the vision needs:
 * Collabora for browser co-editing, a Craft app when you want the real editor.
 */
class AppCatalog {
	/**
	 * slug => [name, mimes, web]
	 *
	 * `web` says whether the app ships a WebAssembly build that can open in a
	 * browser tab. Where it is false, only the desktop handoff is offered.
	 *
	 * MIME types are what Nextcloud reports, so the office formats use the long
	 * OOXML names and the OpenDocument equivalents sit beside them.
	 */
	private const APPS = [
		'photocraft' => [
			'name' => 'PhotoCraft',
			'web' => true,
			'mimes' => [
				'image/png', 'image/jpeg', 'image/webp', 'image/tiff', 'image/bmp',
				'image/vnd.adobe.photoshop', 'image/x-xcf',
			],
		],
		'vectorcraft' => [
			'name' => 'VectorCraft',
			'web' => true,
			'mimes' => ['image/svg+xml', 'application/postscript', 'application/illustrator'],
		],
		'lightcraft' => [
			'name' => 'LightCraft',
			'web' => true,
			// Raw formats. Nextcloud reports most as image/x-dcraw when it cannot
			// be more specific, so that is included on purpose.
			'mimes' => [
				'image/x-dcraw', 'image/x-adobe-dng', 'image/x-canon-cr2', 'image/x-canon-cr3',
				'image/x-nikon-nef', 'image/x-sony-arw', 'image/x-fuji-raf', 'image/x-panasonic-rw2',
			],
		],
		'filmcraft' => [
			'name' => 'FilmCraft',
			'web' => true,
			'mimes' => ['video/mp4', 'video/quicktime', 'video/x-matroska', 'video/webm', 'video/mpeg'],
		],
		'pdfcraft' => [
			'name' => 'PdfCraft',
			'web' => true,
			'mimes' => ['application/pdf'],
		],
		'effectcraft' => [
			'name' => 'EffectCraft',
			'web' => true,
			'mimes' => ['application/x-effectcraft-project'],
		],
		'designcraft' => [
			'name' => 'DesignCraft',
			'web' => true,
			'mimes' => ['application/x-indesign', 'application/x-designcraft-document'],
		],
		'cadcraft' => [
			'name' => 'CADCraft',
			'web' => true,
			'mimes' => ['image/vnd.dxf', 'image/vnd.dwg', 'application/dxf', 'application/acad'],
		],
		'wordcraft' => [
			'name' => 'WordCraft',
			'web' => true,
			'mimes' => [
				'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
				'application/msword',
				'application/vnd.oasis.opendocument.text',
				'application/rtf', 'text/rtf',
			],
		],
		'gridcraft' => [
			'name' => 'GridCraft',
			'web' => true,
			'mimes' => [
				'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
				'application/vnd.ms-excel',
				'application/vnd.oasis.opendocument.spreadsheet',
				'text/csv',
			],
		],
		'deckcraft' => [
			'name' => 'DeckCraft',
			'web' => true,
			'mimes' => [
				'application/vnd.openxmlformats-officedocument.presentationml.presentation',
				'application/vnd.ms-powerpoint',
				'application/vnd.oasis.opendocument.presentation',
			],
		],
		'soundcraft' => [
			'name' => 'SoundCraft',
			'web' => true,
			'mimes' => ['audio/wav', 'audio/x-wav', 'audio/mpeg', 'audio/flac', 'audio/ogg', 'audio/aiff'],
		],
	];

	/**
	 * The catalog as the frontend needs it.
	 *
	 * @return list<array{slug: string, name: string, mimes: list<string>, web: bool}>
	 */
	public function forFrontend(): array {
		$out = [];
		foreach (self::APPS as $slug => $app) {
			$out[] = [
				'slug' => $slug,
				'name' => $app['name'],
				'mimes' => $app['mimes'],
				'web' => $app['web'],
			];
		}

		return $out;
	}

	/** Every MIME any Craft app claims, for a quick membership test. */
	public function allMimes(): array {
		$mimes = [];
		foreach (self::APPS as $app) {
			foreach ($app['mimes'] as $mime) {
				$mimes[$mime] = true;
			}
		}

		return array_keys($mimes);
	}

	/** Which app opens this MIME, or null. */
	public function forMime(string $mime): ?string {
		foreach (self::APPS as $slug => $app) {
			if (in_array($mime, $app['mimes'], true)) {
				return $slug;
			}
		}

		return null;
	}
}
