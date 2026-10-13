/**
 * The "Open in …" entries CollabCraft adds to the Files app.
 *
 * One pair per Craft app that claims the file's MIME type. The web entry opens
 * the WebAssembly build in a tab. The desktop entry hands off to a native
 * install through a URL scheme, which is how the same document moves between
 * browser and native and lands in the same relay room either way.
 *
 * This plugin issues nothing and authorises nothing. It is the one part of the
 * system that cannot be generic, because it lives inside one vendor's file
 * list, so it is kept to exactly that: a link. CollabCraft decides who may open
 * what and signs the token, and it would do the same if this plugin were
 * replaced by an ownCloud or Seafile equivalent.
 *
 * No token travels in a URL. An earlier revision fetched one here and put it in
 * the link, which placed a live credential in browser history, in the Open
 * Recent list and in the desktop process's arguments. The app asks CollabCraft
 * for its own.
 *
 * Neither entry is registered as the default. Collabora owns the default for
 * the office formats through richdocuments, and taking it would change what a
 * double click does for everyone on the instance.
 */

import { registerFileAction, FileAction, Permission } from '@nextcloud/files'
import { loadState } from '@nextcloud/initial-state'
import { translate as t } from '@nextcloud/l10n'

const APP_ID = 'collabcraft'

/** @type {Array<{slug: string, name: string, mimes: string[], web: boolean}>} */
const apps = loadState(APP_ID, 'apps', [])

/**
 * Where CollabCraft lives. Configured on the instance rather than compiled in,
 * so a second deployment needs no rebuild.
 */
const service = (loadState(APP_ID, 'service_url', '') || '').replace(/\/+$/, '')

/**
 * The handle CollabCraft uses for a document.
 *
 * For this provider that is the path relative to the user's files root, which
 * is what WebDAV addresses a file by. It is opaque to everything above the
 * provider, so it is passed through unread.
 */
function handleFor(node) {
	return node.path || `${node.dirname || ''}/${node.basename}`.replace(/\/+/g, '/')
}

/**
 * Whether this node is a file one of the apps claims, and the user may read it.
 *
 * Write permission is deliberately not required. A reader can join, watch edits
 * arrive and see the roster, and the relay refuses their submits. Hiding the
 * entry from readers would make a shared read-only document look unopenable.
 */
function enabledFor(app) {
	return (nodes) => {
		if (nodes.length !== 1) {
			return false
		}
		const node = nodes[0]
		if (node.type !== 'file') {
			return false
		}
		if ((node.permissions & Permission.READ) === 0) {
			return false
		}
		return app.mimes.includes(node.mime)
	}
}

if (!service) {
	// Registering actions that cannot work would be worse than registering
	// none. An administrator sets collabcraft_url.
	console.warn(`[${APP_ID}] no service URL is configured, so no "Open in" entries were added`)
}

for (const app of service ? apps : []) {
	if (app.web) {
		registerFileAction(new FileAction({
			id: `${APP_ID}-web-${app.slug}`,
			displayName: () => t(APP_ID, 'Open in {name}', { name: app.name }),
			iconSvgInline: () => '',
			enabled: enabledFor(app),
			async exec(node) {
				const target = `${service}/${app.slug}/?file=${encodeURIComponent(handleFor(node))}`
				// A new tab, because the editor is a full application and
				// losing the file list to it is not what a single click should
				// do. noopener so the opened page cannot reach back.
				window.open(target, '_blank', 'noopener')
				return true
			},
		}))
	}

	registerFileAction(new FileAction({
		id: `${APP_ID}-native-${app.slug}`,
		displayName: () => t(APP_ID, 'Open in {name} (desktop)', { name: app.name }),
		iconSvgInline: () => '',
		enabled: enabledFor(app),
		async exec(node) {
			// The native install registers this scheme. The link names the app,
			// the document and the service, and nothing else: the desktop app
			// signs in to CollabCraft itself and asks for its own token.
			const deepLink = `collabcraft://open/${app.slug}`
				+ `?file=${encodeURIComponent(handleFor(node))}`
				+ `&name=${encodeURIComponent(node.basename)}`
				+ `&service=${encodeURIComponent(service)}`
			window.location.href = deepLink
			return true
		},
	}))
}
