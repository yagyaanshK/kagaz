#!/usr/bin/env python3
"""Make oui.tsv.gz, the makers of network cards, from the IEEE's public
registry (https://standards-oui.ieee.org/oui/oui.csv, MA-L blocks):
one line per block, "AABBCC<TAB>Maker", sorted, with each maker's name
shortened to what people call it ("Samsung", not "Samsung Electronics
Co.,Ltd"). Nearby on Home shows it for devices Kagaz does not know.

    curl -sSL -o oui.csv https://standards-oui.ieee.org/oui/oui.csv
    python3 -I oui.py oui.csv oui.tsv.gz
"""

import csv
import gzip
import re
import sys

# Names people know, by a word the registered name starts with or holds.
KNOWN = [
    ("apple", "Apple"), ("samsung", "Samsung"), ("xiaomi", "Xiaomi"),
    ("tp-link", "TP-Link"), ("tp link", "TP-Link"), ("google", "Google"),
    ("amazon", "Amazon"), ("espressif", "Espressif"), ("huawei", "Huawei"),
    ("honor device", "Honor"), ("oneplus", "OnePlus"),
    ("guangdong oppo", "Oppo"), ("realme", "Realme"), ("vivo mobile", "Vivo"),
    ("lg electronics", "LG"), ("lg innotek", "LG Innotek"),
    ("sony", "Sony"), ("intel", "Intel"), ("realtek", "Realtek"),
    ("hon hai", "Foxconn"), ("foxconn", "Foxconn"), ("liteon", "Lite-On"),
    ("lite-on", "Lite-On"), ("azurewave", "AzureWave"), ("murata", "Murata"),
    ("raspberry pi", "Raspberry Pi"), ("brother", "Brother"),
    ("hewlett packard", "HP"), ("hp inc", "HP"), ("seiko epson", "Epson"),
    ("canon", "Canon"), ("dell", "Dell"), ("lenovo", "Lenovo"),
    ("asustek", "ASUS"), ("d-link", "D-Link"), ("netgear", "Netgear"),
    ("shenzhen tenda", "Tenda"), ("ubiquiti", "Ubiquiti"), ("cisco", "Cisco"),
    ("nokia", "Nokia"), ("motorola", "Motorola"), ("philips", "Philips"),
    ("signify", "Philips Hue"), ("microsoft", "Microsoft"),
    ("nintendo", "Nintendo"), ("roku", "Roku"), ("sonos", "Sonos"),
    ("panasonic", "Panasonic"), ("sharp", "Sharp"), ("tcl", "TCL"),
    ("hisense", "Hisense"), ("hangzhou hikvision", "Hikvision"),
    ("zhejiang dahua", "Dahua"), ("ezviz", "EZVIZ"), ("tuya", "Tuya"),
    ("chongqing fugui", "Fugui"), ("mediatek", "MediaTek"),
    ("qualcomm", "Qualcomm"), ("broadcom", "Broadcom"),
    ("texas instruments", "Texas Instruments"), ("silicon labs", "Silicon Labs"),
    ("arcadyan", "Arcadyan"), ("sagemcom", "Sagemcom"), ("zte", "ZTE"),
    ("fiberhome", "FiberHome"), ("jio", "Jio"), ("reliance", "Reliance Jio"),
    ("bharti", "Airtel"), ("mercusys", "Mercusys"), ("ruijie", "Ruijie"),
    ("akuvox", "Akuvox"), ("atomberg", "Atomberg"), ("synology", "Synology"),
    ("western digital", "Western Digital"), ("garmin", "Garmin"),
    ("fitbit", "Fitbit"), ("bose", "Bose"), ("harman", "Harman"),
    ("vizio", "Vizio"), ("skyworth", "Skyworth"), ("ring llc", "Ring"),
    ("nest labs", "Google Nest"), ("ecobee", "ecobee"), ("dyson", "Dyson"),
    ("wistron", "Wistron"), ("quanta", "Quanta"), ("compal", "Compal"),
    ("pegatron", "Pegatron"), ("gigabyte", "Gigabyte"), ("micro-star", "MSI"),
    ("acer", "Acer"), ("toshiba", "Toshiba"), ("fujitsu", "Fujitsu"),
    ("hikvision", "Hikvision"), ("imou", "Imou"), ("wyze", "Wyze"),
    ("ezviz", "EZVIZ"), ("lumi united", "Aqara"), ("yeelight", "Yeelight"),
    ("beijing roborock", "Roborock"), ("dreame", "Dreame"),
    ("infinix", "Infinix"), ("tecno mobile", "Tecno"), ("transsion", "Transsion"),
    ("mxchip", "MXCHIP"), ("icomm semiconductor", "iComm"), ("beken", "Beken"),
    ("bouffalo", "Bouffalo Lab"), ("winner micro", "WinnerMicro"),
    ("gaoshengda", "Gaoshengda"), ("amlogic", "Amlogic"),
]

LEGAL = re.compile(
    r"[ ,.]*\b(co\.?|company|corp\.?|corporation|inc\.?|incorporated|ltd\.?|limited|"
    r"llc|l\.l\.c\.|gmbh|ag|s\.?a\.?|s\.?p\.?a\.?|b\.?v\.?|n\.?v\.?|plc|pty|pvt|private|"
    r"oy|ab|as|a/s|kg|k\.k\.|kabushiki kaisha|technologies|technology|tech|"
    r"electronics|electronic|international|holdings|group|industrial|industries|"
    r"communications|communication|systems|network|networks|information|mobility)\b[ ,.]*$",
    re.I,
)


def short(name: str) -> str:
    low = name.lower()
    for key, nice in KNOWN:
        if key in low:
            return nice
    name = re.sub(r"\s+", " ", name).strip(" ,.")
    for _ in range(4):
        cut = LEGAL.sub("", name).strip(" ,.")
        if not cut or cut == name:
            break
        name = cut
    if name.isupper() and len(name) > 4:
        name = " ".join(w if len(w) <= 3 else w.capitalize() for w in name.split())
    return name[:40]


def main() -> None:
    src, out = sys.argv[1], sys.argv[2]
    rows = {}
    with open(src, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            if row.get("Registry") != "MA-L":
                continue
            block = row["Assignment"].strip().upper()
            name = short(row["Organization Name"])
            if len(block) == 6 and name and name.lower() not in ("private", "ieee registration authority"):
                rows[block] = name
    text = "".join(f"{b}\t{n}\n" for b, n in sorted(rows.items()))
    with gzip.GzipFile(out, "wb", compresslevel=9, mtime=0) as g:
        g.write(text.encode("utf-8"))
    print(f"{len(rows)} blocks")


if __name__ == "__main__":
    main()
