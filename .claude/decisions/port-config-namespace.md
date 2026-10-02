# A port owns a config namespace only when its contract requires a config

A port owns a namespace if and only if its contract requires the integrator to
honour a config — the throttler's does, so `<PREFIX>_THROTTLER__*` survives a
backend swap. Otherwise the adapter's config takes the namespace its path gives
it (`SeaOrmConfig` → `<PREFIX>_SEAORM__*`, `RedisWorkerConfig` →
`<PREFIX>_REDIS__WORKER__*`). `<PREFIX>_QUEUE__URL` (a connection filed under
whichever binding asked first) and `<PREFIX>_DATABASE__URL` (naming neither the
crate nor the type that parsed it) were retired by this rule.

Three earlier tests were rejected, so none is re-proposed:

- classifying a field as *policy* or *connection* — a judgement no outsider can
  repeat;
- asking whether *a second driver would write it identically* — a
  counterfactual whose answer depends on the driver imagined;
- reading the namespace off the file's path alone — mechanical, and it does name
  the variable, but it decides nothing about where the field should have been
  declared. The contract decides where; the path then gives the name.
