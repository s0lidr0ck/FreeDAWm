# Recorded kits for Juke

Juke's recorded kits are downloaded from the `kits-v1` release, not kept in the repository
(see `docs/juke.md`). This folder holds the scripts that make them from their sources.

## SM Drums

Source: `SMDrums_Sforzando_1.2.zip` (2.2 GB), linked from
<https://smmdrums.wordpress.com/for-sfz-sforzando/>. SFZ Instruments lists the kit as public
domain (<https://sfzinstruments.github.io/drums/sm_drums>); the authors offer it free of charge
for any use. The zip itself carries no licence file.

```sh
python3 -I tools/kits/build_smdrums.py SMDrums_Sforzando_1.2.zip target/kits/sm_drums
cargo xtask kit-package target/kits/sm_drums --id sm_drums --out target/kits-release
FREEDAWM_TEST_KIT_ZIP=target/kits-release/sm_drums.zip \
  cargo test -p soundcraft-engine --lib -- --ignored a_packed_kit_installs_and_plays
```

The script reads the zip without unpacking it, picks four velocity layers (40, 72, 100, 124) and
four round-robin takes per pad from the SFZ mappings for 16 pieces (kick, side stick, snare,
rimshot, three hi-hats, four toms, two crashes, ride, ride bell, china), trims each take (1.5–6 s)
with a 20 ms fade, and writes `kit.json`. The result is 256 takes, about 216 MB packed. Upload
`sm_drums.zip` and `kits.json` to the `kits-v1` release.
