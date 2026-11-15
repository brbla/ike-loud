# Generátor testovacích vektorů SRP

Vektory v `crates/ikeloud-api/tests/data/srp_vektory.rs` vznikly takto
(pysrp je knihovna, kterou používá pyicloud):

```sh
python3 -m venv /tmp/srp && /tmp/srp/bin/pip install 'srp==1.0.22'
/tmp/srp/bin/python -I vektory.py srp         > vektory.json   # backend _ctsrp (OpenSSL)
/tmp/srp/bin/python -I vektory.py srp._pysrp  > vektory2.json  # čistý Python
cmp vektory.json vektory2.json                                  # musí být shodné
/tmp/srp/bin/python -I okraje.py              > okraje.json
```

`vektory.py` nechá server pysrp ověřit důkaz klienta. `okraje.py` hledá
případy, kdy A nebo S vyjde kratší než 256 bajtů; tam se pysrp a rclone
liší (viz docs/rozhodnuti/0005-prihlaseni-srp.md).

Salt s úvodní nulou vektory záměrně nemají: backend `_ctsrp` ho zkracuje,
`_pysrp` a rclone ne.
