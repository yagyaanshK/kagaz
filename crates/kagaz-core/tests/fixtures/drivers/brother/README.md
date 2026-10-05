# Brother driver lookup recordings

Fetched on 2026-10-05 from `https://download.brother.com/pub/com/linux/linux/infs/`.
This is how Brother's own Linux installer finds packages: `infs/<MODEL>` (the
model name without punctuation, e.g. `DCPL2540DW`) names the driver families,
and `infs/<family>.lnk` names the package file per packaging and architecture.
Packages live under `.../linux/packages/<file>`.

| File | What it is |
|---|---|
| `DCPL2540DW` | the model's inf: printer packages, `SCANNER_DRV=brscan4`, `SCANKEY_DRV=brscan-skey` |
| `brscan4.lnk` | scanner driver package names (DEB32/DEB64/RPM32/RPM64) |
| `brscan-skey.lnk` | scan-key tool package names |
