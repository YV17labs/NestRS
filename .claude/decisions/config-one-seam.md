# A config has one seam, and the loader never arbitrates

`for_root` configures a module; `for_feature` registers a config and takes no
value. This is NestJS's split (`forRoot` configures once, `forFeature` registers
artifacts against an already-configured module). During 7.0 `for_feature`
briefly took a pinned base too, which made every module-owned config reachable
two ways, so its value depended on `imports = [..]` order; it was removed.

Two other loader-side shapes were tried and removed for the same reason — they
put a judgement in the crate that only loads:

- **Ranking the tiers of related variables inside the loader** (half a key pair
  in the deployment, the other half in `.env`). The consumer that uses them now
  refuses an inconsistent combination in its one constructor, naming what is
  set.
- **A second duration reader, `ConfigService::seconds`**, which read connection
  ceilings with no ceiling of their own, beside three hand-written range checks;
  one of them skipped the pinned value and booted a zero Redis budget. Every
  duration is now a `DurationBounds`, refused in the same words from the
  environment and from code.
