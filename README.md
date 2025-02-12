# keyboard chatter fix daemon.

I wrote this since I only saw one project that did this.. in python..

Chattering is the phenomenon of sending multiple key-presses on a keypress of some key, this program alleviates that by adding a delay before the same key can be used again.

This can happen due to wear-over-time, bad firmware, human dexterity loss etc.


## Config example:

`merlijn@mimic ~ % cat $HOME/.config/keyboard-chatter-fix/config.ini`
```ini
# ID can be picked by device name, run `evtest` to pick your device name.
# Or to help with conflicting device names it can also be a linux device-id `ls -al /dev/input/by-id/` and use a file-name in that directory as id
id = usb-Ducky_Ducky_One_3_TKL_RGB_DK-V1.07-220107-if01-event-kbd

# Delay in milliseconds before you can use the same key again
threshold = 30
```

[Systemd service](./systemd/chatter-fix.service)
`systemctl enable --now chatter-fix.service`