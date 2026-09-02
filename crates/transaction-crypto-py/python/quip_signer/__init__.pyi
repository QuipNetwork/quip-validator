class QuipSignerError(Exception):
    """Raised when the Quip hybrid signer rejects its input."""

def public_from_seed(seed: bytes) -> bytes:
    """Derive serialized H4 public bytes (929B) from a 32-byte master seed."""

def account_id_from_public(public: bytes) -> bytes:
    """Derive the compact 32-byte Quip account id from H4 public bytes."""

def seed_from_mnemonic(secret_uri: str) -> bytes:
    """Derive the 32-byte master seed from a BIP39 phrase or `0x`-hex seed URI."""

def sign_payload_from_seed(seed: bytes, payload: bytes) -> bytes:
    """Sign payload bytes with a 32-byte seed; return the SCALE-encoded envelope."""

def verify_envelope(payload: bytes, envelope: bytes, account_id: bytes) -> bool:
    """Verify a SCALE-encoded envelope against a payload and compact account id."""

class HybridSigner:
    @staticmethod
    def from_seed(seed: bytes) -> HybridSigner: ...
    @staticmethod
    def from_mnemonic(secret_uri: str) -> HybridSigner: ...
    @property
    def public_key(self) -> bytes: ...
    @property
    def account_id(self) -> bytes: ...
    def sign(self, payload: bytes) -> bytes: ...
