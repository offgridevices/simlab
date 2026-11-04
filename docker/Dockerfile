# SimLab's reference engines, for checking the built-in solvers:
#   OpenFOAM v2606 - OpenCFD's official runtime image (pinned by digest)
#   CalculiX 2.21  - Ubuntu 26.04's signed package
# Built only when you ask for it (Settings -> System check), run with Colima.
FROM opencfd/openfoam-run:2606@sha256:4229997e74defb81548222d511b8e3b95b98305e5df41b8e88b031813fe47eeb

USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends calculix-ccx \
 && rm -rf /var/lib/apt/lists/* \
 && ccx -v 2>&1 | head -2 || true
# cases are mounted at /case; run as the image's normal user
WORKDIR /case
ENV OMPI_ALLOW_RUN_AS_ROOT=1 OMPI_ALLOW_RUN_AS_ROOT_CONFIRM=1
