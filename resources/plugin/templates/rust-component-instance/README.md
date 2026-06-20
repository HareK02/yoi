# Yoi instance Plugin template

This template targets `yoi:plugin/instance@1.0.0`. The host creates one
`PluginInstance` for the package; Tool, Service, and Ingress surfaces share that
instance state while each surface keeps separate permissions/grants.

Tools still run only through ordinary model/user-initiated Tool calls. Ingress
handlers receive bounded typed untrusted events and must return explicit JSON
for host-mediated visible/durable paths.
