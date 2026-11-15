# SRP test vector generator

The vectors in `crates/ikeloud-api/tests/data/srp_vectors.rs` were made like
this (pysrp is the library pyicloud uses):

```sh
python3 -m venv /tmp/srp && /tmp/srp/bin/pip install 'srp==1.0.22'
/tmp/srp/bin/python -I vectors.py srp         > vectors.json   # _ctsrp backend (OpenSSL)
/tmp/srp/bin/python -I vectors.py srp._pysrp  > vectors2.json  # pure Python
cmp vectors.json vectors2.json                                 # must be identical
/tmp/srp/bin/python -I edges.py               > edges.json
python3 to_rust.py vectors.json edges.json    > ../../crates/ikeloud-api/tests/data/srp_vectors.rs
```

`vectors.py` lets the pysrp server verify the client proof. `edges.py` looks
for cases where A or S comes out shorter than 256 bytes; there pysrp and
rclone differ (see docs/rozhodnuti/0005-prihlaseni-srp.md).

The vectors deliberately have no salt with a leading zero byte: the `_ctsrp`
backend strips it, `_pysrp` and rclone do not.
