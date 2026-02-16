# .NET Runtime on Hyperlight/Unikraft
# Self-contained deployment with Alpine/musl

FROM mcr.microsoft.com/dotnet/sdk:9.0-alpine AS build
WORKDIR /app
COPY Program.cs HelloDotnet.csproj ./
RUN dotnet publish -c Release -r linux-musl-x64 --self-contained \
    -p:PublishTrimmed=true \
    -p:InvariantGlobalization=true \
    -o /out

FROM scratch

# .NET app + runtime (self-contained)
COPY --from=build /out/ /app/

# musl dynamic linker
COPY --from=build /lib/ld-musl-x86_64.so.1 /lib/ld-musl-x86_64.so.1

# C++ runtime libraries (needed by .NET runtime)
COPY --from=build /usr/lib/libstdc++.so.6 /usr/lib/libstdc++.so.6
COPY --from=build /usr/lib/libgcc_s.so.1 /usr/lib/libgcc_s.so.1
