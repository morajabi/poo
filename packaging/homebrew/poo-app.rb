# Homebrew cask template for the menu bar app (bundles its own poo CLI). Published to
# morajabi/homebrew-tap by the release workflow (scripts/homebrew.sh renders it).
cask "poo-app" do
  version "VERSION"
  sha256 "SHA_APP"

  url "https://github.com/morajabi/poo/releases/download/v#{version}/Poo.app.zip"
  name "Poo"
  desc "Menu bar app to inspect and safely free disk space used by dev tools"
  homepage "https://github.com/morajabi/poo"

  auto_updates true
  depends_on macos: :sequoia

  app "Poo.app"
  binary "#{appdir}/Poo.app/Contents/Resources/bin/poo"

  zap trash: [
    "~/.config/poo",
    "~/.local/state/poo",
  ]
end
