<?php

declare(strict_types=1);

namespace OCA\CollabCraft\Controller;

use OCA\CollabCraft\Service\TokenService;
use OCP\AppFramework\Http;
use OCP\AppFramework\Http\Attribute\NoAdminRequired;
use OCP\AppFramework\Http\DataResponse;
use OCP\AppFramework\OCSController;
use OCP\IRequest;
use OCP\IUserSession;

/**
 * The one endpoint. A peer asks for a token for a file id.
 *
 * OCSController rather than ApiController, which upstream marks legacy.
 * NoAdminRequired as an attribute rather than an annotation, since Nextcloud 30
 * deprecated the annotation form.
 */
class TokenController extends OCSController {
	public function __construct(
		string $appName,
		IRequest $request,
		private IUserSession $userSession,
		private TokenService $tokens,
	) {
		parent::__construct($appName, $request);
	}

	/**
	 * Issue a join token, or refuse.
	 *
	 * A missing file and an unshared one both answer 403. Separating them would
	 * tell a caller whether an id exists, which is not theirs to learn.
	 */
	#[NoAdminRequired]
	public function issue(int $fileId): DataResponse {
		$user = $this->userSession->getUser();
		if ($user === null) {
			return new DataResponse(['message' => 'not signed in'], Http::STATUS_UNAUTHORIZED);
		}

		$issued = $this->tokens->issue($user->getUID(), $user->getDisplayName(), $fileId);
		if ($issued === null) {
			return new DataResponse(['message' => 'no access to that file'], Http::STATUS_FORBIDDEN);
		}

		if ($issued['relay'] === '') {
			// A token nobody can use is worse than an honest refusal, because the
			// client would retry against an empty address.
			return new DataResponse(
				['message' => 'no relay configured; set the collabcraft relay_url app value'],
				Http::STATUS_SERVICE_UNAVAILABLE
			);
		}

		return new DataResponse($issued);
	}
}
