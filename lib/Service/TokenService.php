<?php

declare(strict_types=1);

namespace OCA\CollabCraft\Service;

use OCP\AppFramework\Services\IAppConfig;
use OCP\AppFramework\Utility\ITimeFactory;
use OCP\Constants;
use OCP\Files\IRootFolder;
use OCP\Files\Node;
use OCP\Files\NotFoundException;
use OCP\Security\ISecureRandom;

/**
 * Decides who may edit what, then signs that decision.
 *
 * The relay never calls back here. It verifies the signature and trusts the
 * payload, so this class is the only place an access decision is made. One
 * round trip per join, none per edit.
 */
class TokenService {
	/** How long a minted token stays valid. Long enough to join, short enough
	 * that a revoked share stops mattering quickly. */
	private const TTL_SECONDS = 300;

	private const SECRET_KEY = 'relay_secret';
	private const RELAY_KEY = 'relay_url';

	/** Bytes of entropy in the shared secret and in a peer id. */
	private const SECRET_BYTES = 48;
	private const PEER_BYTES = 16;

	public function __construct(
		private IRootFolder $rootFolder,
		private IAppConfig $appConfig,
		private ITimeFactory $timeFactory,
		private ISecureRandom $random,
	) {
	}

	/**
	 * Mint a join token for one user and one file.
	 *
	 * Returns null when the user cannot read the file, which covers a file that
	 * does not exist, one they were never shared, and one whose share was
	 * revoked. The caller turns that into a 403 without distinguishing, because
	 * telling them which would leak whether the id exists.
	 *
	 * @return array{token: string, relay: string, expires: int, peer: string}|null
	 */
	public function issue(string $uid, string $displayName, int $fileId): ?array {
		$node = $this->resolve($uid, $fileId);
		if ($node === null) {
			return null;
		}

		$permissions = $node->getPermissions();
		if (($permissions & Constants::PERMISSION_READ) === 0) {
			return null;
		}

		$expires = $this->timeFactory->getTime() + self::TTL_SECONDS;
		$peer = $this->random->generate(self::PEER_BYTES, ISecureRandom::CHAR_LOWER . ISecureRandom::CHAR_DIGITS);

		// Short keys because this travels in every join. See the README for the
		// field table.
		$payload = [
			'fid' => $fileId,
			'uid' => $uid,
			'dsp' => $displayName,
			// Nextcloud's own bits, unchanged. The relay refuses a submit when
			// PERMISSION_UPDATE is absent, so the share dialog and the right to
			// co-edit stay one source of truth.
			'perm' => $permissions,
			'exp' => $expires,
			'pid' => $peer,
		];

		return [
			'token' => $this->sign($payload),
			'relay' => $this->relayUrl(),
			'expires' => $expires,
			'peer' => $peer,
		];
	}

	/**
	 * The file as this user sees it, or null when they cannot see it.
	 *
	 * getById searches the user's own folder, so a file they have no access to
	 * simply does not come back. That is the permission check, not an
	 * afterthought to it.
	 */
	private function resolve(string $uid, int $fileId): ?Node {
		try {
			$userFolder = $this->rootFolder->getUserFolder($uid);
		} catch (\Throwable $e) {
			return null;
		}

		try {
			$nodes = $userFolder->getById($fileId);
		} catch (NotFoundException $e) {
			return null;
		}

		// A shared file can appear at more than one path. Any of them proves
		// access, so the first is enough.
		return $nodes[0] ?? null;
	}

	/**
	 * Compact `payload.signature`, both base64url without padding.
	 *
	 * Deliberately not a JWT. The relay would need a JWT library to read one,
	 * and this carries six fields with no negotiation, so the format buys
	 * nothing over an HMAC over bytes.
	 */
	private function sign(array $payload): string {
		$encoded = self::b64url((string)json_encode($payload, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE));
		$mac = hash_hmac('sha256', $encoded, $this->secret(), true);

		return $encoded . '.' . self::b64url($mac);
	}

	/**
	 * The secret shared with the relay, generated on first use.
	 *
	 * Marked sensitive so it does not appear in a config dump or a support
	 * report. Rotating it invalidates every live token, which is the intended
	 * revocation path.
	 */
	private function secret(): string {
		// Read with lazy = true, matching how it is written below. A value
		// stored lazy is not visible to a non-lazy read.
		$secret = $this->appConfig->getAppValueString(self::SECRET_KEY, '', true);
		if ($secret !== '') {
			return $secret;
		}

		$secret = $this->random->generate(
			self::SECRET_BYTES,
			ISecureRandom::CHAR_ALPHANUMERIC
		);

		// Mind the signature: setAppValueString(key, value, lazy, sensitive).
		// The third argument is lazy, so passing a single true marks the value
		// lazy and leaves it NOT sensitive, which would let the shared secret
		// appear in a config dump or a support report. Both flags are needed.
		$this->appConfig->setAppValueString(self::SECRET_KEY, $secret, true, true);

		return $secret;
	}

	/** Where peers should connect. Empty until an admin sets it. */
	private function relayUrl(): string {
		return $this->appConfig->getAppValueString(self::RELAY_KEY, '');
	}

	/** base64url, unpadded, as used by JWS. */
	private static function b64url(string $raw): string {
		return rtrim(strtr(base64_encode($raw), '+/', '-_'), '=');
	}
}
