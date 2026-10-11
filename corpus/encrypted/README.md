Encrypted variants of `../hello.pdf`, made with qpdf 12.2.0. The owner password is
`owner-pw`. `*-user.pdf` files have the user password `user-pw`; `*-empty.pdf` files have an
empty user password (owner-only).

    qpdf --allow-weak-crypto --encrypt "$U" owner-pw 40 -- hello.pdf rc4-40-*.pdf          # R2
    qpdf --allow-weak-crypto --encrypt "$U" owner-pw 128 --use-aes=n -- ... rc4-128-*.pdf  # R3
    qpdf --encrypt "$U" owner-pw 128 --use-aes=y -- ... aes-128-*.pdf                      # R4
    qpdf --encrypt "$U" owner-pw 256 -- ... aes-256-*.pdf                                  # R6
