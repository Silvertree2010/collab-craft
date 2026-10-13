<?php

declare(strict_types=1);

// One endpoint. A peer asks for a token for a file id, and either gets one or
// gets told no. Everything after that happens against the relay.
//
// OCS rather than a plain route, so the response carries the standard envelope
// every other Nextcloud API uses and a client needs no special casing.
return [
    'ocs' => [
        [
            'name' => 'Token#issue',
            'url' => '/api/v1/token/{fileId}',
            'verb' => 'GET',
            'requirements' => ['fileId' => '\d+'],
        ],
    ],
];
