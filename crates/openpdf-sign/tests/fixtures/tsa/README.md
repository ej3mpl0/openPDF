Test-only RFC 3161 TSA material for openpdf-sign tests. These keys are public
and must never be trusted outside the test suite.

- root.*: test TSA root CA (trusted by the tests).
- tsa.*: TSA leaf signed by root (EKU timeStamping, critical).
- evilroot.* / eviltsa.*: an unrelated CA and a TSA leaf with the same subject,
  used to check that an untrusted TSA chain is rejected.

Keys are PKCS#8 DER, certificates DER. Generated with OpenSSL 3, 20-year validity.

- openssl-token-essv2.der / openssl-token-essv1.der: RFC 3161 tokens made by
  `openssl ts -reply` with tsa.* over openssl-token-data.bin (ESS signing
  certificate v2 / v1), to check interoperability with a real TSA implementation.

Extra certificates from `gen-extra.sh` (leaves reuse tsa.key, intermediates
inter.key; all chain to root):

- tsa-eku-noncrit / tsa-eku-multi / tsa-eku-none: EKU not critical, EKU with
  timeStamping plus serverAuth, no EKU (all must be refused, RFC 3161 §2.3).
- inter-nokcs + tsa-under-nokcs: intermediate CA without keyCertSign.
- inter-pl0 (pathLen 0) + tsa-under-pl0 (allowed) and inter2 + tsa-under-inter2
  (a CA below a pathLen 0 CA: refused).
- filler-01..12: ~3.8 KB certificates that push a token over the 32 KiB
  reserved for a B-T signature.
