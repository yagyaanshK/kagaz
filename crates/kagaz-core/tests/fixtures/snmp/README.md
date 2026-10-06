# SNMP recordings

Raw UDP payloads exchanged with a Brother DCP-L2540DW (198.51.100.167,
firmware "NC-8300w Ver.Z") on 2026-10-05, community `public`.

| File | What it is |
|---|---|
| `brother-dcp-l2540dw.v2c.request.bin` | SNMP v2c GetRequest, request-id 0x4b41, for sysDescr, sysObjectID, sysName, hrDeviceDescr, prtGeneralPrinterName, prtGeneralSerialNumber, ppmPrinterIEEE1284DeviceId, prtMarkerLifeCount, prtInterpreterLangFamily.1, hrPrinterStatus.1, hrDeviceStatus.1 |
| `brother-dcp-l2540dw.v2c.response.bin` | Its GetResponse: every OID answered except prtGeneralPrinterName (noSuchObject). The same bytes came back whether the request went to the printer's address or to the subnet broadcast address. |
| `brother-dcp-l2540dw.v1-rejected.response.bin` | The GetResponse to the same request sent as SNMP v1: error-status 2 (noSuchName), error-index 5, no values. This is why the v1 probe asks for sysDescr only. |

The printer's serial number, hostname and address were replaced by
placeholders of the same length (KAGAZ0000000001, BRWEXAMPLE00001,
198.51.100.167); everything else is as recorded.
