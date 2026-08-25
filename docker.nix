{
  pkgs,
  yoi ? pkgs.callPackage ./package.nix { },
}:

let
  lib = pkgs.lib;

  imageTag = "latest";

  mkRoot =
    name: paths: _extraPathsToLink:
    pkgs.symlinkJoin {
      inherit name paths;
    };

  runtimeDirs = pkgs.runCommand "yoi-runtime-dirs" { } ''
    mkdir -p "$out/runtime-data" "$out/workdirs"
    chmod 0777 "$out/runtime-data" "$out/workdirs"
  '';

  serverDirs = pkgs.runCommand "yoi-server-dirs" { } ''
    mkdir -p "$out/server-data" "$out/workspace"
    chmod 0777 "$out/server-data" "$out/workspace"
  '';

  runtimeRoot =
    mkRoot "yoi-runtime-root"
      [
        yoi
        pkgs.bashInteractive
        pkgs.coreutils
        pkgs.cacert
        pkgs.git
        pkgs.openssh
        runtimeDirs
      ]
      [
        "/runtime-data"
        "/workdirs"
      ];

  serverRoot =
    mkRoot "yoi-server-root"
      [
        yoi
        pkgs.coreutils
        pkgs.cacert
        pkgs.git
        serverDirs
      ]
      [
        "/server-data"
        "/workspace"
      ];

  webuiSrc = lib.cleanSourceWith {
    src = ./web/workspace;
    filter =
      path: type:
      let
        baseName = baseNameOf path;
      in
      !(baseName == "node_modules" || baseName == ".svelte-kit" || baseName == "build");
  };

  webuiDeps = pkgs.stdenvNoCC.mkDerivation {
    pname = "yoi-webui-deno-deps";
    version = "0.1.0";

    src = webuiSrc;
    nativeBuildInputs = [ pkgs.deno ];

    outputHashAlgo = "sha256";
    outputHashMode = "recursive";
    outputHash = "sha256-q+otr+ANR9gB8bRZKFZDfNpM6rnQ4B4wNd/s1QNeSA4=";

    buildPhase = ''
      runHook preBuild

      export HOME="$TMPDIR/home"
      export DENO_DIR="$TMPDIR/deno-cache"
      mkdir -p "$HOME" "$DENO_DIR"
      deno task build

      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall

      mkdir -p "$out"
      cp -R "$DENO_DIR" "$out/deno-cache"
      if [ -d node_modules ]; then
        cp -R node_modules "$out/node_modules"
      fi

      runHook postInstall
    '';
  };

  webuiStatic = pkgs.stdenvNoCC.mkDerivation {
    pname = "yoi-webui-static";
    version = "0.1.0";

    src = webuiSrc;

    nativeBuildInputs = [ pkgs.deno ];

    buildPhase = ''
      runHook preBuild

      export HOME="$TMPDIR/home"
      export DENO_DIR="$TMPDIR/deno-cache"
      mkdir -p "$HOME"
      cp -R ${webuiDeps}/deno-cache "$DENO_DIR"
      chmod -R u+w "$DENO_DIR"
      if [ -d ${webuiDeps}/node_modules ]; then
        cp -R ${webuiDeps}/node_modules node_modules
        chmod -R u+w node_modules
      fi
      deno task build

      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall

      mkdir -p "$out"
      cp -R build/. "$out/"

      runHook postInstall
    '';
  };

  webuiRoot = pkgs.runCommand "yoi-webui-root" { } ''
    mkdir -p "$out/usr/share/yoi-webui"
    cp -R ${webuiStatic}/. "$out/usr/share/yoi-webui/"
  '';

  webuiDirs = pkgs.runCommand "yoi-webui-dirs" { } ''
    mkdir -p "$out/etc" "$out/tmp" "$out/var/cache/nginx" "$out/var/log/nginx"
    touch "$out/tmp/.keep" "$out/var/cache/nginx/.keep" "$out/var/log/nginx/.keep"
    cat > "$out/etc/passwd" <<'EOF'
    root:x:0:0:root:/root:/bin/sh
    nobody:x:65534:65534:nobody:/var/empty:/sbin/nologin
    EOF
    cat > "$out/etc/group" <<'EOF'
    root:x:0:
    nobody:x:65534:
    nogroup:x:65534:
    EOF
  '';

  nginxConf = pkgs.writeText "yoi-webui-nginx.conf" ''
    pid /tmp/nginx.pid;
    error_log /dev/stderr info;

    events {}

    http {
      include ${pkgs.nginx}/conf/mime.types;
      access_log /dev/stdout;

      map $http_upgrade $connection_upgrade {
        default upgrade;
        ''' close;
      }

      server {
        listen 80;
        server_name _;
        root /usr/share/yoi-webui;
        index index.html;

        resolver 127.0.0.11 valid=30s ipv6=off;
        set $yoi_backend server:8787;

        location = /api {
          proxy_pass http://$yoi_backend;
          proxy_http_version 1.1;
          proxy_set_header Host $host;
          proxy_set_header X-Real-IP $remote_addr;
          proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
          proxy_set_header X-Forwarded-Proto $scheme;
          proxy_set_header Upgrade $http_upgrade;
          proxy_set_header Connection $connection_upgrade;
        }

        location /api/ {
          proxy_pass http://$yoi_backend;
          proxy_http_version 1.1;
          proxy_set_header Host $host;
          proxy_set_header X-Real-IP $remote_addr;
          proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
          proxy_set_header X-Forwarded-Proto $scheme;
          proxy_set_header Upgrade $http_upgrade;
          proxy_set_header Connection $connection_upgrade;
        }

        location / {
          try_files $uri $uri/ /index.html;
        }
      }
    }
  '';

  webuiImageRoot =
    mkRoot "yoi-webui-root-env"
      [
        pkgs.nginx
        pkgs.cacert
        webuiRoot
        webuiDirs
      ]
      [
        "/usr/share/yoi-webui"
        "/var"
        "/tmp"
      ];

  commonEnv = [
    "SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt"
    "GIT_SSL_CAINFO=/etc/ssl/certs/ca-bundle.crt"
  ];
in
{
  runtime = pkgs.dockerTools.buildImage {
    name = "yoi-runtime";
    tag = imageTag;
    copyToRoot = runtimeRoot;
    config = {
      Entrypoint = [ "/bin/yoi-runtime" ];
      Cmd = [
        "--bind"
        "0.0.0.0:38800"
        "--display-name"
        "Docker Runtime"
        "--fs-root"
        "/runtime-data"
        "--workdir-target"
        "/workdirs"
      ];
      Env = [ "PATH=/bin" ] ++ commonEnv;
      ExposedPorts = {
        "38800/tcp" = { };
      };
      Volumes = {
        "/runtime-data" = { };
        "/workdirs" = { };
      };
      WorkingDir = "/runtime-data";
    };
  };

  server = pkgs.dockerTools.buildImage {
    name = "yoi-server";
    tag = imageTag;
    copyToRoot = serverRoot;
    config = {
      Entrypoint = [ "/bin/yoi-server" ];
      Cmd = [
        "serve"
        "--listen"
        "0.0.0.0:8787"
        "--config"
        "/server-config/server.toml"
      ];
      Env = [
        "PATH=/bin"
        "YOI_DATA_DIR=/server-data"
      ] ++ commonEnv;
      ExposedPorts = {
        "8787/tcp" = { };
      };
      Volumes = {
        "/server-data" = { };
        "/server-config" = { };
      };
      WorkingDir = "/server-data";
    };
  };

  webui = pkgs.dockerTools.buildImage {
    name = "yoi-webui";
    tag = imageTag;
    copyToRoot = webuiImageRoot;
    config = {
      Entrypoint = [
        "/bin/nginx"
        "-c"
        "${nginxConf}"
        "-g"
        "daemon off;"
      ];
      Env = [ "PATH=/bin" ] ++ commonEnv;
      ExposedPorts = {
        "80/tcp" = { };
      };
    };
  };

  webui-static = webuiStatic;
}
