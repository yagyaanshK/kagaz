# WSD-Scan recordings

HTTP bodies exchanged with a Brother DCP-L2540DW (198.51.100.167, firmware Z)
on 2026-10-05, over its hosted scanner service at
`http://198.51.100.167:80/WebServices/ScannerService`.

| File | What it is |
|---|---|
| `brother-dcp-l2540dw.metadata.response.xml` | WS-Transfer Get on the device endpoint: model, serial, and the hosted PrinterService and ScannerService with their own addresses |
| `brother-dcp-l2540dw.getscannerelements.response.xml` | GetScannerElements: configuration (formats, resolutions, colours, sizes for glass and feeder), description, default ticket, status |
| `brother-dcp-l2540dw.createscanjob.response.xml` | CreateScanJob for a 2 x 2 inch region of the glass at 100 dpi, colour, exif: job id 1 and a token |
| `brother-dcp-l2540dw.retrieveimage.response.bin` | RetrieveImage for that job: the `Content-Type` header line, a blank line, then the raw MTOM multipart body with the SOAP part and the JPEG. The JPEG bytes were replaced by a synthetic 176 x 189 checkerboard of the same dimensions; the multipart framing, headers and SOAP part are as recorded |
| `brother-dcp-l2540dw.retrieveimage-after-last.response.xml` | RetrieveImage once more on the same job: HTTP 400 with fault `wscn:ClientErrorJobIdNotFound`. This is how the end of a job looks. |
| `brother-dcp-l2540dw.createscanjob-adf-empty.response.xml` | CreateScanJob on the feeder with no paper in it: after about ten seconds, HTTP 500 with fault `wscn:ServerErrorNotAcceptingJobs`. This is how an empty feeder looks on this model. |

Nothing personal is in these files
beyond the printer's serial number, which it gives to anyone on the network.
