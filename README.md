# MailCatcher

![CI](https://github.com/lolo32/mailcatcher/workflows/Ci/badge.svg)
![Security audit](https://github.com/lolo32/mailcatcher/workflows/Security%20audit/badge.svg)
[![codecov](https://codecov.io/gh/lolo32/mailcatcher/branch/main/graph/badge.svg?token=DH3JmaeWaZ)](https://codecov.io/gh/lolo32/mailcatcher)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

## What can I do, what's that?

This software can be used to have a SMTP mail server on any computer that
accept any mail and display them using any web browser.

It DOES NOT really send them to any remote recipient address.

It acts as a SMTP server that can be used on port 1025 (_25 for default 
port_), and a web server that display the deceived mail on port 1080 (_80 
for default port_).

## Status

***It's a work in progress***…

_Developed with the help of JetBrains 
[CLion](https://www.jetbrains.com/clion/)._

## License

It's published under the _[Apache License Version 2.0](LICENSE.txt)_.

The CSS is the [W3.CSS](https://www.w3schools.com/w3css/) that _does not have 
a license and is free to use_.

The Javascript is [Hyperapp](https://github.com/jorgebucaran/hyperapp) that 
is licensed under the _[MIT](https://mit-license.org/)_ license.

All rust used libraries are licensed under [MIT](https://mit-license.org/) 
except `broadcaster` that is unlicensed; some are also licensed under 
[Apache License Version 2.0](LICENSE.txt).
