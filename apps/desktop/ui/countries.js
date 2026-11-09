// Countries and regions with their calling codes, for phone-number fields
// (the OneTouch "New Family Member" form's Country / Region). Names come from
// the system's own locale data (Intl.DisplayNames), so they read in the
// user's language; the code itself is what the services want ("91").

const KagazCountries = (() => {
  // ISO 3166-1 alpha-2 code and ITU calling code.
  const CODES = "AF93 AL355 DZ213 AS1684 AD376 AO244 AI1264 AG1268 AR54 AM374 AW297 AU61 AT43 AZ994 BS1242 BH973 BD880 BB1246 BY375 BE32 BZ501 BJ229 BM1441 BT975 BO591 BA387 BW267 BR55 VG1284 BN673 BG359 BF226 BI257 KH855 CM237 CA1 CV238 KY1345 CF236 TD235 CL56 CN86 CO57 KM269 CG242 CD243 CK682 CR506 CI225 HR385 CU53 CW599 CY357 CZ420 DK45 DJ253 DM1767 DO1809 EC593 EG20 SV503 GQ240 ER291 EE372 SZ268 ET251 FK500 FO298 FJ679 FI358 FR33 GF594 PF689 GA241 GM220 GE995 DE49 GH233 GI350 GR30 GL299 GD1473 GP590 GU1671 GT502 GG44 GN224 GW245 GY592 HT509 HN504 HK852 HU36 IS354 IN91 ID62 IR98 IQ964 IE353 IM44 IL972 IT39 JM1876 JP81 JE44 JO962 KZ7 KE254 KI686 XK383 KW965 KG996 LA856 LV371 LB961 LS266 LR231 LY218 LI423 LT370 LU352 MO853 MG261 MW265 MY60 MV960 ML223 MT356 MH692 MQ596 MR222 MU230 YT262 MX52 FM691 MD373 MC377 MN976 ME382 MS1664 MA212 MZ258 MM95 NA264 NR674 NP977 NL31 NC687 NZ64 NI505 NE227 NG234 NU683 KP850 MK389 MP1670 NO47 OM968 PK92 PW680 PS970 PA507 PG675 PY595 PE51 PH63 PL48 PT351 PR1787 QA974 RE262 RO40 RU7 RW250 BL590 SH290 KN1869 LC1758 MF590 PM508 VC1784 WS685 SM378 ST239 SA966 SN221 RS381 SC248 SL232 SG65 SX1721 SK421 SI386 SB677 SO252 ZA27 KR82 SS211 ES34 LK94 SD249 SR597 SE46 CH41 SY963 TW886 TJ992 TZ255 TH66 TL670 TG228 TK690 TO676 TT1868 TN216 TR90 TM993 TC1649 TV688 UG256 UA380 AE971 GB44 US1 UY598 VI1340 UZ998 VU678 VA39 VE58 VN84 WF681 YE967 ZM260 ZW263";
  let names = null;
  try { names = new Intl.DisplayNames([navigator.language || "en"], { type: "region" }); } catch (_) {}
  const list = CODES.split(" ").map((t) => {
    const iso = t.slice(0, 2), code = t.slice(2);
    let name = iso;
    try { name = (names && names.of(iso)) || iso; } catch (_) {}
    return { iso, code, name };
  }).sort((a, b) => a.name.localeCompare(b.name));

  // The likely country: the one chosen last time, else the system's region.
  function guess() {
    try { const last = localStorage.getItem("kagaz-phone-country"); if (last && list.some((c) => c.iso === last)) return last; } catch (_) {}
    const region = String(navigator.language || "").split("-")[1];
    return region && list.some((c) => c.iso === region.toUpperCase()) ? region.toUpperCase() : "";
  }
  function remember(iso) { try { localStorage.setItem("kagaz-phone-country", iso); } catch (_) {} }

  // A <select> of every country, "India (+91)", with the likely one chosen.
  function select(id) {
    const s = document.createElement("select");
    if (id) s.id = id;
    const none = document.createElement("option"); none.value = ""; none.textContent = "Choose…";
    s.appendChild(none);
    for (const c of list) {
      const o = document.createElement("option"); o.value = c.iso; o.textContent = `${c.name} (+${c.code})`; o.dataset.code = c.code;
      s.appendChild(o);
    }
    s.value = guess();
    return s;
  }
  const codeOf = (iso) => (list.find((c) => c.iso === iso) || {}).code || "";

  return { list, select, codeOf, remember };
})();
