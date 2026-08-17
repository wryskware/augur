Output pwsh.gif
Output pwsh.mp4
Set Shell "pwsh"
Set FontSize 26
Set Width 1200
Set Height 640
Set Padding 32
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Hide
Type `cd ~/augur-gif/demo; $env:AUGUR_HOME='/home/steven/augur-gif/rec-pwsh'; $env:AUGUR_CONFIG='/home/steven/augur-gif/rec-pwsh/config.toml'; $env:PATH='/home/steven/augur-build/target/release:'+$env:PATH; function prompt { "PS demo> " }; clear`
Enter
Sleep 2s
Show
Sleep 600ms
Type "ask list all the files here recursively"
Sleep 400ms
Enter
Sleep 3.2s
Sleep 1.4s
