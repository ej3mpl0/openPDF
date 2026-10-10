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
