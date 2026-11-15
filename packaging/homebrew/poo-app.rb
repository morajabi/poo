# Homebrew cask template for the menu bar app (bundles its own poo CLI).
cask "poo-app" do
  version "VERSION"
  sha256 "SHA_APP"

  url "https://github.com/morajabi/poo/releases/download/v#{version}/Poo.app.zip"
  name "Poo"
  desc "Menu bar app to inspect and safely free disk space used by dev tools"
  homepage "https://github.com/morajabi/poo"

  depends_on macos: ">= :sonoma"
  auto_updates true

  app "Poo.app"
  binary "#{appdir}/Poo.app/Contents/Resources/bin/poo"

  zap trash: [
    "~/.config/poo",
    "~/.local/state/poo",
  ]
end
