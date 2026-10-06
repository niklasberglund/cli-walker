# Records docs/demo.gif against a simulated pad and sample history.
# Requires VHS (brew install vhs). Run from the repo root:
#
#   ./gradlew macosArm64Binaries && vhs demo/demo.tape

Output docs/demo.gif

Set Shell zsh
Set FontSize 16
Set Width 1020
Set Height 900
Set Padding 20
Set TypingSpeed 60ms

Hide
Type 'alias cli-walker="./build/bin/macosArm64/debugExecutable/cli-walker.kexe --demo"' Enter
Type "clear" Enter
Show

Type "cli-walker" Sleep 400ms Enter
Sleep 4s

# Speed up
Up Sleep 1.5s
Up Sleep 3s

# Statistics
Tab Sleep 4s

# Strava: lower the upload threshold and mute the feed
Tab Sleep 1.5s
Down Sleep 600ms
Down Sleep 600ms
Down Sleep 1.2s
Type "m" Sleep 2.5s

# Back to the session, then quit
Tab Sleep 2.5s
Type "q"
Sleep 4s
