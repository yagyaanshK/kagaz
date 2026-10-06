# IPP recordings

Raw IPP request and response bodies (RFC 8010 encoding) exchanged with a
Brother DCP-L2540DW at `ipp://198.51.100.167:631/ipp/print` on 2026-10-05.

| File | What it is |
|---|---|
| `brother-dcp-l2540dw.get-printer-attributes.request.bin` | Get-Printer-Attributes (0x000B), request-id 0x4b41, requested-attributes `all` |
| `brother-dcp-l2540dw.get-printer-attributes.response.bin` | Its reply: 6 KB, every printer attribute including the `media-col-*` collections and the single `BK` toner marker |
| `brother-dcp-l2540dw.get-jobs.request.bin` | Get-Jobs (0x000A), which-jobs `not-completed`, requested-attributes `all` |
| `brother-dcp-l2540dw.get-jobs.response.bin` | Its reply with an empty queue: only the operation attributes group |

The printer's hostname, address and the device part of its UUID were
replaced by placeholders of the same length; everything else is as recorded.
