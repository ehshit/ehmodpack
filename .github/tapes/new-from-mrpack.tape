Output new-from-mrpack.gif

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
Type "ls"
Sleep 2s
Enter
Wait
Sleep 4s

Sleep 1s
Type "ehmodpack new-from-mrpack fabulously.mrpack --workflow source --force"
Sleep 2s
Enter
Wait+Screen /visit your project/
Sleep 5s

Sleep 1s
Type "ls"
Sleep 2s
Enter
Wait
Sleep 4s

Sleep 1s
Type "clear"
Sleep 2s
Enter
Sleep 5s