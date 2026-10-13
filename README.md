# CollabCraft

Cloud documents and shared editing for the Craft apps.

One service holds four jobs: signing people in, reaching whatever cloud their
documents live in, relaying live edits between them, serving the browser
builds. They are together because they share a secret, a bus and a provider.
Splitting them would mean three deployments agreeing on all three.

An app talks only to CollabCraft. It holds no provider credential, knows no
provider URL and implements no provider protocol. That is the whole design: a
desktop binary carrying a cloud password leaks one the moment somebody copies
the file. A plugin per provider reimplements authentication, file authorisation
and token minting every time.

## What it needs

| Dependency | Why | Notes |
|---|---|---|
| A Redis-compatible server | per-document sequence, scope locks, sessions | Valkey or Redis. Must not evict: see below |
| An OIDC provider | signing people in | Tested against Authentik and Keycloak |
| A cloud provider | where documents live | Nextcloud today. See *Providers* |
| A reverse proxy | TLS | Optional, but the OIDC redirect wants https |

Nothing else. The service writes nothing to disk and keeps no database of its
own.

**The bus must not evict.** The sequence counters are not a cache. Losing one
silently reorders every edit in that document. Clients then hold their edits,
waiting for numbers that never arrive. Run it with
`--maxmemory-policy noeviction`. The bundled compose file already does.

## Deploying

```sh
cp .env.example .env
$EDITOR .env          # every variable is documented in place
docker compose up -d
curl -fsS http://127.0.0.1:9444/healthz
```

`/healthz` answers only once the bus is writable. A service that is up but
cannot order an edit therefore fails the check.

Register a confidential OIDC client with the redirect URI
`${COLLABCRAFT_PUBLIC_URL}/auth/callback` and the scopes `openid profile email
groups`. Entitlement is decided from group membership, so without `groups`
every user looks ungrouped.

### Behind an internal certificate authority

A stock image trusts neither an internal identity provider nor an internal
cloud provider. The failure reads as *could not reach the identity provider*.
That says nothing about a certificate. Mount a bundle carrying the
public roots **alongside** your own CA and point `COLLABCRAFT_CA_BUNDLE` at it.
Replacing the system bundle with only your CA breaks every public call.

```sh
cat /etc/ssl/certs/ca-certificates.crt your-root-ca.crt > ca-bundle.pem
```

## Providers

`craft-cloud` defines what a provider offers: files, sharing, contacts,
notifications, mail, accounts. None of it names a vendor. None of it has to. Files are WebDAV, contacts are CardDAV, calendars are CalDAV. Only
sharing and notification are vendor-shaped. Both are small.

`craft-nextcloud` is one implementation. A second is an implementation of six
traits, with no change to this service and none to any app.

What a provider can actually do is reported, never assumed. An administrator
who forbids public links gets no Copy link button. The alternative is a button
that fails on use.

## Entitlement

`COLLABCRAFT_APPS_DEFAULT` is what everyone who signs in may open.
`COLLABCRAFT_APPS_<GROUP>` adds to that for members of a group from the
identity provider. Serving an app directory checks it. Adding somebody to a group is therefore the
whole of granting access.

A client is never asked whether it may run. A client asking itself is no check
at all.

## The browser builds

Point `COLLABCRAFT_WEB_DIR` at a directory of per-app WebAssembly builds, one
directory per app. They are served behind the same permission check as
everything else.

Serve them precompressed: put `foo.wasm.br` beside `foo.wasm` and the service
sends it with the right `Content-Encoding`. Brotli at a high level costs
minutes per bundle. That is unaffordable per request and free once at deploy.
A 16 MB bundle becomes about 5 MB.

Leave the directory empty to run as an API and relay only.

## How an edit travels

The API asks the provider whether this user may open this document, signs a
short-lived token carrying the answer, then hands it over. The relay verifies
that signature locally and never calls back. Authorisation costs one round trip
per document, not one per keystroke.

Each edit gets a sequence number from the bus and is fanned out in that order.
Concurrent work in different regions proceeds freely; concurrent work in the
same region is serialised by a scope lock. That is deliberately short of a
CRDT. A CRDT would need stable identity for positions and a rewrite of each
app's document model.

## Licence

See `LICENSE-APACHE` and `LICENSE-MIT`.
