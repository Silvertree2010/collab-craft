<?php

declare(strict_types=1);

namespace OCA\CollabCraft\Listener;

use OCA\CollabCraft\AppInfo\Application;
use OCA\CollabCraft\Service\AppCatalog;
use OCA\Files\Event\LoadAdditionalScriptsEvent;
use OCP\AppFramework\Services\IAppConfig;
use OCP\AppFramework\Services\IInitialState;
use OCP\EventDispatcher\Event;
use OCP\EventDispatcher\IEventListener;
use OCP\Util;

/**
 * Puts the "Open in …" entries into the Files app.
 *
 * This listener is the whole of the Nextcloud side. It issues nothing and
 * authorises nothing, because CollabCraft does both. What is left here is the
 * one part that cannot be generic: putting a link in this vendor's file list.
 *
 * @template-implements IEventListener<LoadAdditionalScriptsEvent>
 */
class LoadFilesScripts implements IEventListener {
	/** Where CollabCraft is. Configured per instance, never compiled in. */
	public const SERVICE_KEY = 'service_url';

	public function __construct(
		private IInitialState $initialState,
		private IAppConfig $appConfig,
		private AppCatalog $catalog,
	) {
	}

	public function handle(Event $event): void {
		if (!($event instanceof LoadAdditionalScriptsEvent)) {
			return;
		}

		// The catalog travels as initial state so the script has the MIME map
		// before it registers anything, and so there is only ever one copy of
		// that map.
		$this->initialState->provideInitialState('apps', $this->catalog->forFrontend());
		$this->initialState->provideInitialState(
			'service_url',
			$this->appConfig->getAppValueString(self::SERVICE_KEY, ''),
		);

		// addInitScript, not addScript. Upstream added it in 28 precisely to
		// avoid the race where a file action registers after the Files app has
		// already read its action list.
		//
		// Only when the bundle exists. The source imports @nextcloud/files and
		// has to be bundled before a browser can load it. Registering a missing
		// asset would put a 404 on every Files page load and still give nobody
		// the menu entries, so staying silent is the honest behaviour.
		//
		// The name here has no extension and the file on disk ends in .js,
		// because Util::addInitScript appends it. An earlier revision tested
		// for a .mjs that nothing ever produced, so the entries never appeared
		// and nothing said why.
		if (!file_exists(__DIR__ . '/../../js/collabcraft-files.js')) {
			return;
		}
		Util::addInitScript(Application::APP_ID, 'collabcraft-files');
	}
}
