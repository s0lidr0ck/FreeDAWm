# Recorded kits for Juke

Juke's recorded kits are downloaded from the `kits-v1` release, not kept in the repository
(see `docs/juke.md`). This folder holds the scripts that make them from their sources.

## SM Drums

Source: `SMDrums_Sforzando_1.2.zip` (2.2 GB), linked from
<https://smmdrums.wordpress.com/for-sfz-sforzando/>. SFZ Instruments lists the kit as public
domain (<https://sfzinstruments.github.io/drums/sm_drums>); the authors offer it free of charge
for any use. The zip itself carries no licence file.

The same source makes three kits; the script's third argument picks one (default `sm_drums`):

| Kit | id | Pieces |
|---|---|---|
| SM Drums | `sm_drums` | 6.5" snare, rimshot, hats, four toms, 16" and 13" crashes, 20" ride and bell, china |
| SM Drums 17 | `sm_drums_17` | the pieces SM Drums leaves out: 6.7" snare, a dry 6.5", pedal-open, loose and half-open hats, 15" and 17" crashes, 17" ride and bell |
| SM Drums Hybrid | `sm_drums_hybrid` | three hybrid snares and their rimshot over the kit (the hybrid and dry snares have two takes per layer) |

```sh
python3 -I tools/kits/build_smdrums.py SMDrums_Sforzando_1.2.zip target/kits/sm_drums sm_drums
cargo xtask kit-package target/kits/sm_drums --id sm_drums --out target/kits-release
FREEDAWM_TEST_KIT_ZIP=target/kits-release/sm_drums.zip \
  cargo test -p soundcraft-engine --lib -- --ignored a_packed_kit_installs_and_plays
```

The script reads the zip without unpacking it, picks four velocity layers (40, 72, 100, 124) and
four round-robin takes per pad from the SFZ mappings for 16 pieces (kick, side stick, snare,
rimshot, three hi-hats, four toms, two crashes, ride, ride bell, china), trims each take (1.5–6 s)
with a 20 ms fade, and writes `kit.json`. The result is 256 takes, about 216 MB packed.

`.github/workflows/kits.yml` does all of this for each kit on GitHub Actions (download, build, pack, check) and
creates or updates the `kits-v1` release; run it from Actions › Kits, or merge a change here
into `main`.
