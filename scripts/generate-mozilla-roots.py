import argparse
import base64
import csv
import hashlib
import io
from pathlib import Path
import urllib.request

REPORT = "https://ccadb.my.salesforce-sites.com/mozilla/IncludedCACertificateReportPEMCSV"
OUTPUT = Path(__file__).resolve().parent.parent / "crates/core/platform/src/trust/mozilla_roots.der"
PEM_BEGIN = "-----BEGIN CERTIFICATE-----"
PEM_END = "-----END CERTIFICATE-----"
TLS_TRUST_BIT = "Websites"

parser = argparse.ArgumentParser()
parser.add_argument("--report", type=Path, help="a saved copy of the CCADB report instead of downloading it")
parser.add_argument("--output", type=Path, default=OUTPUT)
args = parser.parse_args()
if args.report:
    text = args.report.read_text(encoding="utf-8")
else:
    with urllib.request.urlopen(REPORT) as response:
        text = response.read().decode("utf-8")
roots = {}
for row in csv.DictReader(io.StringIO(text, newline="")):
    if TLS_TRUST_BIT not in row["Trust Bits"].split(";"):
        continue
    pem = row["PEM Info"].strip().strip("'")
    if not pem.startswith(PEM_BEGIN) or not pem.endswith(PEM_END):
        raise ValueError(f"{row['Common Name or Certificate Name']}: no PEM certificate")
    der = base64.b64decode("".join(pem[len(PEM_BEGIN):-len(PEM_END)].split()), validate=True)
    fingerprint = row["SHA-256 Fingerprint"].upper()
    if hashlib.sha256(der).hexdigest().upper() != fingerprint:
        raise ValueError(f"{row['Common Name or Certificate Name']}: fingerprint does not match the certificate")
    roots[fingerprint] = der
if not roots:
    raise ValueError("the report lists no roots trusted for TLS")
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_bytes(b"".join(roots[fingerprint] for fingerprint in sorted(roots)))
print(f"{len(roots)} roots, {args.output.stat().st_size} bytes in {args.output}")
