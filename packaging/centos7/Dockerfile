FROM quay.io/centos/centos:7.9.2009

ARG RUST_VERSION=1.97.1

RUN sed -i 's/^mirrorlist=/#mirrorlist=/' /etc/yum.repos.d/CentOS-*.repo \
    && sed -i 's|^#baseurl=http://mirror.centos.org|baseurl=http://vault.centos.org|' /etc/yum.repos.d/CentOS-*.repo \
    && yum clean all \
    && yum -y install ca-certificates curl gcc gcc-c++ make pkgconfig binutils file which \
       tar gzip sed findutils cpio rpm-build rpmdevtools \
       libX11-devel libXcursor-devel libXi-devel libXrandr-devel mesa-libGL-devel \
       libxkbcommon-devel dbus-devel xdg-utils \
    && yum clean all

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION} \
    && cargo install mdbook --version 0.4.52 --locked

WORKDIR /workspace
CMD ["/bin/bash", "/workspace/scripts/build-centos7.sh", "2.2.1"]
