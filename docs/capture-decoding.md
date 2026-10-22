# Captured transport bodies

Observer forwards request and response bytes and their content headers unchanged. Inspection happens afterward, in the single capture writer, while the existing capture permit remains held. It does not add a network buffer, per-request decompressor task, or decode queue.

Live captures retain bounded internal transport metadata. Imported JSON and legacy captures keep their existing parsed-JSON contract; they do not accept binary body encodings through this feature.

## Inspection formats and limits

Identity JSON keeps the existing JSON-or-null normalization behavior. Encoded JSON supports one complete `gzip`/`x-gzip` member or one standard `zstd` frame. The decoder checks completion and checksums when the format supplies them. It rejects additional members or frames, trailing bytes, dictionaries, skippable or magicless zstd frames, and multiple or ambiguous content encodings.

Both the raw body and the decoded body must fit `--capture-limit` independently: 256 KiB by default, up to 16 MiB. The zstd window is limited to 8 MiB. A fixed scratch buffer detects decoded overflow before allocating past the output limit. Request and response decoding is sequential. The output limit is not a total process memory bound: codec state, JSON values, existing normalization copies, queued raw captures and database work also consume memory.

Request `multipart/form-data` inspection supports a first `request` part containing a JSON object, followed by uniquely named PNG or JPEG parts referenced once each by the original JSON `assets` map. Parts can use an image MIME type, `application/octet-stream`, or no MIME type; declared image types must agree with the signature. No file paths or URLs are opened, and filenames are not retained. This recognizes image signatures without decoding pixels or validating image dimensions.

Multipart inspection is limited to 128 parts, a 70-byte boundary, 1 KiB part names, 16 KiB of headers per part, and 64 KiB of headers total. Repeated names, missing or unclaimed parts, shared part references, nested multipart, unsupported part headers/encodings, incomplete framing and ambiguous asset descriptors produce an incomplete capture. These are inspection limits; they do not reject or alter the forwarded request.

With input capture disabled, original JSON still reaches secret discovery before known input paths are removed. Binary parts are not converted to base64. With input capture enabled, each referenced image receives bounded `data_base64` and, if absent, a signature-derived `format`; the original `part` metadata remains for provenance. Existing conflicting `format` or `data_base64` fields are not overwritten. This retained inspection representation is not a guaranteed SDK replay request. References in definitions require the model layer's separate semantic isolation rules.

## Failure and compatibility evidence

Unsupported, malformed, incomplete or over-limit encoded content produces a minimal incomplete record with a static decoding reason. Raw bytes, part names and parser errors are excluded from that message. The writer counts a newly incomplete record once; captures already truncated by the proxy keep their existing single gap count. HTTP status, headers and body bytes are unaffected.

Codec and multipart unit tests cover exact limits, checksums, completion, ambiguity and privacy ordering. Proxy tests compare exact raw request/response bytes and verify admission recovery after failure. The current SDK fixture pins Python 0.7.5 (with its zstd extra) and JavaScript 0.7.0, exercising small identity JSON, large JSON with default zstd, compressed multipart and explicit compression disablement against a synthetic loopback provider. The previous Python 0.7.1/JavaScript 0.6.0 fixture remains a separate compatibility check. No fixture calls a real provider or loads model weights.
