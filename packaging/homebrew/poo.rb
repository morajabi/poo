# Homebrew formula template. `scripts/homebrew.sh <version>` fills in urls and sha256
# from the release's latest.json; the release workflow publishes it to
# morajabi/homebrew-tap. Submit to homebrew-core once poo meets its notability bar.
class Poo < Formula
  desc "Safely free disk space eaten by dev builds, worktrees, caches and simulators"
  homepage "https://github.com/morajabi/poo"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/morajabi/poo/releases/download/vVERSION/poo-aarch64-apple-darwin.tar.gz"
      sha256 "SHA_AARCH64_DARWIN"
    end
    on_intel do
      url "https://github.com/morajabi/poo/releases/download/vVERSION/poo-x86_64-apple-darwin.tar.gz"
      sha256 "SHA_X86_64_DARWIN"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/morajabi/poo/releases/download/vVERSION/poo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "SHA_AARCH64_LINUX"
    end
    on_intel do
      url "https://github.com/morajabi/poo/releases/download/vVERSION/poo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "SHA_X86_64_LINUX"
    end
  end

  def install
    bin.install "poo"
  end

  def caveats
    <<~EOS
      Choose your dev folders:            poo setup
      Teach your coding agents about poo: poo integration install all
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/poo --version")
    system bin/"poo", "mods", "validate", "--text"
  end
end
