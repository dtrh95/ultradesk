{
  callPackage,
  inputs,
}:
(callPackage ../ultradesk-rust { }).overrideAttrs (old: {
  pname = "ultradesk-rust-head";
  version = "0-unstable";
  src = inputs.self;
})
