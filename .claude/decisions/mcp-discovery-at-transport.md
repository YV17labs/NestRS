# MCP discovery is gated at the transport, as the specification places it

`initialize`, `tools/list` and `prompts/list` are gated only by the endpoint's
`check_http`. This was once recorded as a hole; the specification says it is the
design. MCP "provides authorization capabilities at the transport level", the
server "acts as an OAuth 2.1 resource server", and "authorization MUST be
included in every HTTP request from client to server", with 401 for an absent or
invalid token and 403 for insufficient scope. Nothing in it authorizes a
JSON-RPC method individually.

The GraphQL comparison does not transfer. `_service` and `_entities` are gated in
band because GraphQL has no transport-level authorization to be uniform over —
one POST carries an arbitrary document, so the field is the addressable unit.
MCP's unit is the HTTP request. A `check_mcp` chain over discovery would be a
layer above the standard; build it if a product asks, never carry it as a debt.

A related fail-open was closed in 7.0: the MCP chain once excluded the global
pool from the operation site, so a global guard overriding only `check_mcp` was
never consulted while its presence disarmed the deny-all empty-pool tail. An
endpoint guard's `check_http` and an operation's `check_mcp` are two questions,
and neither answers the other.
