<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-compiler — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-COMPILER-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

Next code: `G-ORBITAL-COMPILER-002`

- **G-ORBITAL-COMPILER-001** [mechanical] — A routed listen's `when` and `with` read `?field` from the event it hears, and `orb validate` never checks those reads against that event's payload: `A.PING -> GOT when (> ?zzz 0) with { size: ?zzz }` validates 0/0 although PING declares only `size`, and so does `$MOUNT -> SEED_PAGE when (> (array/len ?missing) 0)` against the kernel's `$MOUNT { page, route, traits, entities }` (found 2026-11-04 adding the `$MOUNT` listen). The read is null at run time. Owners: orbital-compiler validation (resolve the heard event's payload: the source trait's declared emit, or the kernel's `$MOUNT` shape from orbital-core) or the std listen lints. Repro: the two probes above under `.orbital/probe/`. Prevention rung: (2) `orb validate`. Root cause: A2.
