# Certificates

`tplink-ca.pem`: the certificate chain TP-Link's cloud (`*.tplinkcloud.com`,
`*.tplinknbu.com`, the relay servers) presents, including their private
"TP-LINK CA P1" root. It is the public bundle the Tapo app ships (its
`mergedCA.pem`) and is trusted in addition to the system roots when Kagaz
talks to TP-Link's cloud. Public certificates only; nothing secret.
