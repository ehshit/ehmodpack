Output new-from-mrpack.gif

Require ehmodpack

Set Shell bash

Set Theme "Catppuccin Mocha"
Set FontSize 16
Set Width 1280
Set Height 720
Set Framerate 12
Set PlaybackSpeed 3
Set LoopOffset 20%
Set Margin 15
Set MarginFill "#657BF2"
Set Padding 10
Set BorderRadius 10
Set WindowBar Colorful
Set CursorBlink false
Set TypingSpeed 25ms
Set WaitTimeout 300s

Sleep 2s

Sleep 500ms
Type "ls"
Sleep 1s
Enter
Wait
Sleep 2s

Sleep 500ms
Type "ehmodpack new-from-mrpack fabulously.mrpack --workflow source --force"
Sleep 1s
Enter
Wait+Screen /Created a modpack for/
Sleep 3s

Hide
Type "clear"
Sleep 1s
Enter
Show
Sleep 2s