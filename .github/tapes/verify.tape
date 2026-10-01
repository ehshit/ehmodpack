Output verify.gif

Require ehmodpack

Set Shell bash

Set Theme "Catppuccin Mocha"
Set FontSize 16
Set Width 1280
Set Height 720
Set Framerate 12
Set PlaybackSpeed 2
Set LoopOffset 20%
Set Margin 15
Set MarginFill "#657BF2"
Set Padding 10
Set BorderRadius 10
Set WindowBar Colorful
Set CursorBlink false
Set TypingSpeed 200ms
Set WaitTimeout 120s

Sleep 3s

Sleep 1s
Type "ehmodpack verify --keep-going"
Sleep 2s
Enter
Wait+Screen /[Ff]ix these/
Sleep 3s

Type "y"
Enter
Wait+Screen /wrote ehmodpack.lock.json/
Sleep 4s

Sleep 1s
Type "cls"
Sleep 2s
Enter
Sleep 5s