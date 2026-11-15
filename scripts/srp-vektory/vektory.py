import hashlib, json, sys
import importlib
backend = sys.argv[1]
srp = importlib.import_module(backend)
srp.rfc5054_enable()
srp.no_username_in_x()
N = int("AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B855F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773BCA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB694B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73", 16)
g = 2
def l2b(n): return n.to_bytes((n.bit_length() + 7) // 8, "big")

def heslo(password, salt, iterace, protokol):
    h = hashlib.sha256(password.encode()).digest()
    if protokol == "s2k_fo":
        h = h.hex().encode()
    return hashlib.pbkdf2_hmac("sha256", h, salt, iterace, 32)

pripady = [
    ("tester@example.com", "heslo123", bytes(range(1, 17)), 20309, "s2k", 0x11, 0x22),
    ("tester@example.com", "heslo123", bytes(range(1, 17)), 20309, "s2k_fo", 0x11, 0x22),
    ("Jana.Nováková@example.cz", "Příliš žluťoučký kůň 🐎", b"\x9asalt-bez-nul", 1000, "s2k", 0x7f, 0x01),
]
vystup = []
for ucet, pw, salt, it, prot, sa, sb in pripady:
    p = heslo(pw, salt, it, prot)
    a = bytes([sa]) * 32
    b = bytes([sb]) * 32
    if backend == "srp._pysrp":
        a = bytes(224) + a
        b = bytes(224) + b
    x = int.from_bytes(hashlib.sha256(salt + hashlib.sha256(b":" + p).digest()).digest(), "big")
    v = l2b(pow(g, x, N))
    usr = srp.User(ucet, p, hash_alg=srp.SHA256, ng_type=srp.NG_2048, bytes_a=a)
    _, A = usr.start_authentication()
    ver = srp.Verifier(ucet, salt, v, A, hash_alg=srp.SHA256, ng_type=srp.NG_2048, bytes_b=b)
    s, B = ver.get_challenge()
    M1 = usr.process_challenge(s, B)
    HAMK = ver.verify_session(M1)
    assert HAMK is not None, "server M1 neuznal"
    usr.verify_session(HAMK)
    assert usr.authenticated()
    vystup.append(dict(ucet=ucet, heslo=pw, salt=salt.hex(), iterace=it, protokol=prot,
        a=a[-32:].hex(), b=b[-32:].hex(), heslo_pbkdf2=p.hex(), A=A.hex(), B=B.hex(),
        M1=M1.hex(), M2=usr.H_AMK.hex(), K=usr.get_session_key().hex()))
json.dump(vystup, sys.stdout, indent=1, ensure_ascii=False)
