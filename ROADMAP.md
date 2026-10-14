# CollabCraft roadmap

Hard record of what is done, what is broken, and what is owed. Ordered by what
blocks what.

## Status, 2026-10-12

Deployed and reachable. The co-edit path does **not** work.

| Piece | State |
|---|---|
| Relay | running, `/healthz` 200, bus ready |
| `collabcraft` Nextcloud app | enabled on 30.0.17 |
| Shared secret | in the deployment's secret store, pushed to both consumers |
| Token minting | works, verified against a real file id |
| Two peers co-editing | **fails**, six consecutive runs |

Run `smoke-test.sh` for a PASS or FAIL verdict in one command.

## 0. BLOCKER: on_connect fires twice per client

Confirmed across six runs: two `peer joined` lines per peer id, 60 to 80 ms
apart. The submit is then dropped with no error logged anywhere.

Two fixes have already been tried and ruled out. Handler registration was
moved ahead of every await. The container's DNS was pinned. Neither changed
the failure.

Next diagnostic step: log `socket.id` alongside the peer id. That distinguishes
one socket connecting twice from two sockets existing for one client. Two
sockets would mean the client's packets land on one while the handlers sit on
the other. That matches the symptom exactly.

Nothing below is worth building until a submit reliably crosses the relay.

## 1. Make every integration a contract

**This was asked for and not done.** The current shape is wrong and needs
correcting before more features land on top of it.

What exists today abstracts the **transport** and hardcodes the **provider**.
`craft-nextcloud` has an `Http` trait so a caller supplies the transport.
`craft-collab` has a `Hub` trait for the same reason. WebDAV, the OCS Share
API and CardDAV are then called directly, under a crate named for one vendor.

What it should be:

* `craft-sync` (or similar) defines the contracts and nothing else. `Files`,
  `Sharing`, `Contacts`, `Calendar`, `Notifications`, `Mail` as traits, with
  plain data types and no vendor in sight.
* `craft-collab` ingests through those traits and stays an intermediary. It
  should not know Nextcloud exists.
* `craft-nextcloud` becomes one implementation of those traits, moving from
  the centre to the edge.

Nothing about CardDAV, WebDAV or CalDAV is Nextcloud-specific. They are RFCs.
An ownCloud, a Seafile or a plain SabreDAV server could all provide them. The
OCS Share API is the only genuinely vendor-shaped piece. Tying the lot to one
vendor is the mistake.

The Nextcloud app half then reads as what it is: a plugin supplying providers
for that implementation.

## 2. Per-app patches

Each needs a fork. The shared pieces belong in `craft-ui` once; the per-app
work is wiring each app's existing button to them.

### Share dialog

`wordcraft/crates/ui-egui/src/ribbon.rs:157` hardcodes Share to
`ui.backstage {"page": "export"}`. It is an export affordance with no provider
behind it. GridCraft and DeckCraft have their own equivalents.

What it needs is a dialog carrying a contact picker over the `Contacts`
contract, permission checkboxes mapping to the share bits, and a note field.
The picker and the dialog live in `craft-ui`. Each app then gets a one-line
change, pointing Share at the dialog where it currently opens export.

### Discord to Talk

Located in the source, with line numbers:

| App | Location |
|---|---|
| wordcraft | `ui-egui/src/chrome.rs:167-189`, a toolbar button running `ui.discord` |
| wordcraft | `crates/mcp/src/server.rs:138`, inside the MCP instructions string |
| gridcraft | `ui-egui/src/dialogs.rs:1331` |
| gridcraft | `apps/gridcraft/src/native_menu.rs:177` |

The target should be configurable. Swapping one hardcoded URL for another
leaves the next instance stuck, where configuration lets each point at its own
Talk.

### Mail and notifications

Both are Nextcloud surfaces reached through the contracts in item 1. Sharing
already raises a Nextcloud notification. Part of this therefore arrives free
once shares go through the provider. A share-with-message email needs the
`sendMail` parameter. An in-app "X is editing with you" needs the
Notifications API from the app side.

## 3. Browser peer

`apps/wordcraft-web` already builds the full editor in WebAssembly. No WASM
work is owed. What is missing sits either side of it.

A `Hub` over the JS `socket.io-client` through `wasm-bindgen`. `rust_socketio`
0.6.0 depends on `native-tls` unconditionally and cannot target wasm32. That
comes from reading its manifest.

A bundle for `js/collabcraft-files.js`. It is ES module source importing
`@nextcloud/files`. No browser can load that as written. The listener in
`LoadFilesScripts.php` checks for the built file and stays silent without it.
Installing the app today therefore produces no broken asset and no menu
entries either.

## 4. Launcher

`storytold-ref/craft-launcher` is mirrored. Self-hosting needs two changes.

**Endpoints must come from the environment.** Today they are compiled in:
`https://api.github.com/repos/storytold/<app>/releases/latest` and
`getartcraft.com`. A self-hosted instance needs its own Forgejo, CDN and site.
One useful seam exists already. `crates/core/src/feed.rs:566` reads
`CRAFT_APPS_FEED_SAMPLE` from a file path. That test hook is a shape a real
feed override could follow.

**The app list must be configurable.** It is a fixed set today. Adding
`solidcraft` (`github.com/bherbruck/solidcraft`) belongs in config, where it
costs a line instead of a patch. That app is third-party and sits outside the
`storytold` org. It is absent from the current mirror set, and convention gives
it its own `<vendor>-ref` org.

## 5. Something to test against locally

The twelve apps are installed on the workstation and none is patched. No client
exists in any of them. The only client is the `join` example.

A patched build is the real unblocker. GridCraft is the right first target,
since its commands already carry their own cell address where WordCraft's act
on the local caret.

Note the build constraint. The workstation has cargo and no MSVC linker. A
Windows build therefore needs either the C++ build tools installed or a
cross-compile. Linux builds go in a container.

## 6. The web face

Built. The service renders its own pages out of the one binary.

| Page | What it answers |
|---|---|
| `/` | the applications this signed-in user may open |
| `/account` | who they are, whether storage is linked, what they may open |
| `/settings/provider` | which cloud this deployment uses |
| `/settings/apps` | what it hosts, and who may open each one |

No framework, no build step, no external asset. The launcher takes over from a
hand-written `index.html` in the web root. That file published an internal
path to anybody who read it.

The cloud provider is configurable from a browser now. The environment stays
the default. A value saved on the page overrides it. Membership of a group
named by `COLLABCRAFT_ADMIN_GROUPS` is the whole of the permission to change
it. Saving tests the address first.

Entitlement works the same way. A deployment grants an application to a group
from a browser. Nobody waits for a redeploy and nobody loses a session. The
page lists what this deployment hosts beside what it grants. It names anything
granted that nobody deployed. That is the one mistake whose only symptom is a
card that never appears.

What is still owed here is the share dialog's own web surface. A browser build
carrying `craft-ui::share` is the piece that makes sharing reachable without a
desktop application.

## Not forgotten

The relay's healthcheck was wrong and is fixed: it used `/dev/tcp`, a bash
feature, against an image whose `/bin/sh` is dash.
