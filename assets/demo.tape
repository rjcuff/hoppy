# Regenerate with: vhs assets/demo.tape
# Output comes from assets/demo.sh: real format, made-up addresses.
Output assets/demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 1280
Set Height 440
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 55ms

Hide
Type `PS1='$ '; hoppy() { bash assets/demo.sh "$@"; }; clear`
Enter
Sleep 2s
Show

Type "hoppy"
Sleep 400ms
Enter
Sleep 5s

Type "clear"
Enter
Type "hoppy port 3000 --kill"
Sleep 400ms
Enter
Sleep 2s
Type "y"
Enter
Sleep 4s

Type "clear"
Enter
Type "hoppy doctor github.com"
Sleep 400ms
Enter
Sleep 6s
