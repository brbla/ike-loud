# Edge-case vectors: short A or short S. The "pysrp" variant comes from the
# srp library, the "rclone" variant follows backend/iclouddrive/api/srp.go
# (everything padded to 256 bytes).
import hashlib, json, sys
import srp._pysrp as srp

srp.rfc5054_enable()
srp.no_username_in_x()
N = int("AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B855F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773BCA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB694B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73", 16)
g = 2
H = lambda *d: hashlib.sha256(b"".join(d)).digest()
l2b = lambda n: n.to_bytes((n.bit_length() + 7) // 8, "big")
pad = lambda n: n.to_bytes(256, "big")
account, salt = "tester@example.com", bytes(range(1, 17))
p = hashlib.pbkdf2_hmac("sha256", hashlib.sha256(b"heslo123").digest(), salt, 1000, 32)
x = int.from_bytes(H(salt, H(b":" + p)), "big")
v = pow(g, x, N)
k = int.from_bytes(H(l2b(N), pad(g)), "big")


def rclone(a, B):
    A = pow(g, a, N)
    u = int.from_bytes(H(pad(A), pad(B)), "big")
    S = pow(B - k * v, a + u * x, N)
    K = H(pad(S))
    hxor = bytes(i ^ j for i, j in zip(H(pad(g)), H(l2b(N))))
    M1 = H(hxor, H(account.encode()), salt, pad(A), pad(B), K)
    return dict(A=pad(A).hex(), M1=M1.hex(), M2=H(pad(A), M1, K).hex(), K=K.hex())


def pysrp(a, b):
    usr = srp.User(account, p, hash_alg=srp.SHA256, ng_type=srp.NG_2048, bytes_a=a.to_bytes(256, "big"))
    _, A = usr.start_authentication()
    ver = srp.Verifier(account, salt, l2b(v), A, hash_alg=srp.SHA256, ng_type=srp.NG_2048,
                       bytes_b=b.to_bytes(256, "big"))
    s, B = ver.get_challenge()
    M1 = usr.process_challenge(s, B)
    HAMK = ver.verify_session(M1)
    assert HAMK is not None
    usr.verify_session(HAMK)
    return dict(A=A.hex(), M1=M1.hex(), M2=usr.H_AMK.hex(), K=usr.get_session_key().hex()), B


def S_of(a, B):
    A = pow(g, a, N)
    u = int.from_bytes(H(pad(A), pad(B)), "big")
    return pow(B - k * v, a + u * x, N)


out = []
# short A
a = next(a for a in range(1, 10**6) if pow(g, a, N).bit_length() <= 2040 and a > 300)
r_py, B = pysrp(a, 0x22)
Bn = int.from_bytes(B, "big")
out.append(dict(name="short A", a=a.to_bytes(32, "big").hex(), B=B.hex(), pysrp=r_py, rclone=rclone(a, Bn)))
# short S (A and B full length)
a = 0x1111
for b in range(1, 10**6):
    r_py, B = pysrp(a, b)
    Bn = int.from_bytes(B, "big")
    if Bn.bit_length() > 2040 and S_of(a, Bn).bit_length() <= 2040:
        break
out.append(dict(name="short S", a=a.to_bytes(32, "big").hex(), B=B.hex(), pysrp=r_py, rclone=rclone(a, Bn)))
json.dump(dict(account=account, salt=salt.hex(), derived=p.hex(), cases=out), sys.stdout, indent=1)
