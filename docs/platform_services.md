# Platform service bridge

Stasis applications use `src/stdlib/platform_services.stasis` for optional host
capabilities whose result arrives asynchronously, such as billing, rewarded ads,
achievements, or cloud saves. The bridge transports opaque service/action IDs and does
not embed any provider SDK or provider-specific policy in the compiler or runtime.

## Contract

- Requests contain positive service, action, and request IDs plus a non-empty printable
  ASCII key of at most 128 bytes.
- At most 16 requests may be outstanding. A duplicate request ID is invalid and a full
  queue returns `PLATFORM_SERVICE_SUBMIT_BUSY`.
- Responses are delivered in publication order and contain the correlated IDs, a
  bounded status, one `i32` value, and at most 512 bytes of validated UTF-8 text.
- Each native request also carries an opaque, process-monotonic dispatch token. Host
  callbacks complete that token rather than guest IDs, so a callback from a reset
  session cannot complete a newly reused request ID.
- Polling is non-blocking. An undersized output buffer returns `-1` without consuming
  the response.
- A host with no registered adapter accepts the request and queues an explicit
  `PLATFORM_SERVICE_RESPONSE_UNSUPPORTED` response.
- Queue reset clears outstanding work but preserves the registered host adapter.

Request and response queues are native, bounded, and synchronized for callbacks from a
platform UI thread. Platform adapters must still marshal UI APIs onto the thread their
SDK requires. Guest code should submit from menus or lifecycle transitions and poll at
a stable application boundary before deterministic simulation consumes the result.

## Host adapter API

`runtime/stasis_platform_services.h` defines the C adapter boundary:

- `stasis_platform_service_set_handler` registers or removes one host dispatcher;
- `stasis_platform_service_publish_response` completes a pending dispatch token from
  either a synchronous handler or a later platform callback;
- `stasis_platform_service_reset` discards pending work between guest sessions.

Handlers return accepted, unsupported, or failed dispatch status. Unsupported and
failed dispatches become responses automatically. A handler that accepts work owns
publishing exactly one later response; duplicate and unknown completions are rejected.
The request pointer is call-scoped, so an asynchronous handler must copy the request,
including its dispatch token, before returning.

The bridge is transport, not durable entitlement storage. An adapter that receives a
valuable platform callback must persist its provider-owned grant before publishing the
response, and the application must record its own entitlement before acknowledging or
consuming that grant.
