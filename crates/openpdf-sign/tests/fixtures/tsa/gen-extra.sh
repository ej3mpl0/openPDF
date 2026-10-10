#!/bin/sh
# Regenerates the extra TSA test certificates (see README.md). Test-only.
# Leaves reuse tsa.key.der; intermediates share inter.key.der.
set -eu
cd "$(dirname "$0")"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
openssl pkey -inform der -in root.key.der -out "$t/root.pem"
openssl pkey -inform der -in tsa.key.der -out "$t/tsa.pem"
[ -f inter.key.der ] || openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -outform der -out inter.key.der
openssl pkey -inform der -in inter.key.der -out "$t/inter.pem"
openssl x509 -inform der -in root.cert.der -out "$t/root.crt"
serial=100
# mk NAME KEY ISSUER_CRT ISSUER_KEY "EXTENSIONS"
mk() {
  serial=$((serial + 1))
  printf '%b\n' "$5" > "$t/$1.ext"
  openssl req -new -key "$t/$2.pem" -subj "/CN=openPDF Test $1" -out "$t/$1.csr"
  openssl x509 -req -in "$t/$1.csr" -CA "$t/$3.crt" -CAkey "$t/$4.pem" -set_serial $serial \
    -days 7300 -sha256 -extfile "$t/$1.ext" -out "$t/$1.crt"
  openssl x509 -in "$t/$1.crt" -outform der -out "$1.cert.der"
}
LEAF='basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,nonRepudiation\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid'
CA='basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid'
mk tsa-eku-noncrit tsa root root "$LEAF\nextendedKeyUsage=timeStamping"
mk tsa-eku-multi tsa root root "$LEAF\nextendedKeyUsage=critical,timeStamping,serverAuth"
mk tsa-eku-none tsa root root "$LEAF"
mk inter-nokcs inter root root 'basicConstraints=critical,CA:TRUE\nkeyUsage=critical,digitalSignature\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid'
mk tsa-under-nokcs tsa inter-nokcs inter "$LEAF\nextendedKeyUsage=critical,timeStamping"
mk inter-pl0 inter root root 'basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid'
mk tsa-under-pl0 tsa inter-pl0 inter "$LEAF\nextendedKeyUsage=critical,timeStamping"
mk inter2 inter inter-pl0 inter "$CA"
mk tsa-under-inter2 tsa inter2 inter "$LEAF\nextendedKeyUsage=critical,timeStamping"
# Bulky, unrelated certificates to overflow the reserved /Contents space.
pad=$(head -c 3000 /dev/zero | tr '\0' 'x')
rm -f filler-*.cert.der
for i in 01 02 03 04 05 06 07 08 09 10 11 12; do
  mk filler-$i tsa root root "$LEAF\nnsComment=$pad"
done
