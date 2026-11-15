# Turns vectors.json and edges.json into the Rust test data module.
import json, sys

v = json.load(open(sys.argv[1]))
e = json.load(open(sys.argv[2]))
q = lambda s: json.dumps(s, ensure_ascii=False)

print('''//! SRP test vectors generated with pysrp 1.0.22 (the `_ctsrp` and `_pysrp`
//! backends gave identical results and the pysrp server accepted the proof).
//! Edge cases for the rclone variant computed following `api/srp.go`.
//! Generator: scripts/srp-vectors/.

pub struct Vector {
    pub account: &'static str,
    pub password: &'static str,
    pub salt: &'static str,
    pub iterations: u32,
    pub protocol: &'static str,
    pub a: &'static str,
    pub derived: &'static str,
    pub public_a: &'static str,
    pub b: &'static str,
    pub m1: &'static str,
    pub m2: &'static str,
    pub k: &'static str,
}

pub const VECTORS: &[Vector] = &[''')
for x in v:
    print(f'''    Vector {{
        account: {q(x["account"])},
        password: {q(x["password"])},
        salt: "{x["salt"]}",
        iterations: {x["iterations"]},
        protocol: "{x["protocol"]}",
        a: "{x["a"]}",
        derived: "{x["derived"]}",
        public_a: "{x["A"]}",
        b: "{x["B"]}",
        m1: "{x["M1"]}",
        m2: "{x["M2"]}",
        k: "{x["K"]}",
    }},''')
print('];')
print(f'''
pub struct Expected {{
    pub public_a: &'static str,
    pub m1: &'static str,
    pub m2: &'static str,
    pub k: &'static str,
}}

pub struct Edge {{
    pub name: &'static str,
    pub a: &'static str,
    pub b: &'static str,
    pub pysrp: Expected,
    pub rclone: Expected,
}}

pub const EDGE_ACCOUNT: &str = {q(e["account"])};
pub const EDGE_SALT: &str = "{e["salt"]}";
pub const EDGE_DERIVED: &str = "{e["derived"]}";

pub const EDGES: &[Edge] = &[''')
for c in e["cases"]:
    def r(d):
        return f'Expected {{ public_a: "{d["A"]}", m1: "{d["M1"]}", m2: "{d["M2"]}", k: "{d["K"]}" }}'
    print(f'''    Edge {{
        name: {q(c["name"])},
        a: "{c["a"]}",
        b: "{c["B"]}",
        pysrp: {r(c["pysrp"])},
        rclone: {r(c["rclone"])},
    }},''')
print('];')
