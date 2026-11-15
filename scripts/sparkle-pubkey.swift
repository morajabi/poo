// Reads a base64 Ed25519 seed (Sparkle's exported private key) on stdin and prints
// the base64 public key Sparkle expects in SUPublicEDKey.
import CryptoKit
import Foundation

let input = String(decoding: FileHandle.standardInput.readDataToEndOfFile(), as: UTF8.self)
guard let seed = Data(base64Encoded: input.trimmingCharacters(in: .whitespacesAndNewlines)), seed.count == 32,
      let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed) else {
    FileHandle.standardError.write(Data("not a base64 32-byte Sparkle private key (generate_keys -x exports one)\n".utf8))
    exit(1)
}
print(key.publicKey.rawRepresentation.base64EncodedString())
