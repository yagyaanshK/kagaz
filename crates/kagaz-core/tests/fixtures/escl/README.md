# eSCL documents

These are NOT recordings. No eSCL-capable scanner was available when the
eSCL client was written, so these documents were hand-built from the eSCL
(Mopria Scan / AirScan) specification and the shapes sane-airscan accepts:
a two-source scanner with a duplex feeder, discrete resolutions, three
colour modes, JPEG and PDF formats.

| File | What it is |
|---|---|
| `spec-example.scannercapabilities.xml` | `GET /eSCL/ScannerCapabilities` |
| `spec-example.scannerstatus-adf-loaded.xml` | `GET /eSCL/ScannerStatus` with paper in the feeder |
| `spec-example.scannerstatus-adf-empty.xml` | the same with an empty feeder |

Replace them with real recordings (and keep these as the spec baseline)
once an eSCL device has been scanned from; see the WSD fixtures for the
naming scheme.
