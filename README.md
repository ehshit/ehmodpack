# Eh's Modpacks

![](/.github/tapes/readme.gif)

<div align="center">

<a href="https://github.com/ehshit/ehmodpack/releases">
  <img src="https://img.shields.io/github/v/release/ehshit/ehmodpack" alt="Latest Release"></a>
  <a href="#"><img src="https://img.shields.io/github/issues/ehshit/ehmodpack?style=flat&color=orange"></a>

</div>

Does Nothing then create ModPacks for Modrinth by making it as a project, You can read more in the [Wiki](https://docs.ehis.gay/stuff/ehmodpack/) for anything that it has

# What does it do then that?

## Resource pack Managing
![](/.github/tapes/order.gif)

You can manage the order of the resource packs for your version(s) so it applies the right ones when building the pack!

## It can create from Modrinth Modpacks
![](/.github/tapes/new-from-mrpack.gif)

If you ever feel that you want to move the modpack you created to Eh's Modpacks you can! (including the configuration of your modpack with `--include-configs`)

*(However obviously if this is NOT your project make sure to comply with the project licenses and [Modrinth's Content Rules](https://modrinth.com/legal/rules))*

## It can verify your content
![](/.github/tapes/verify.gif)

But not also verify, apply the right fixes!!!

When verify is ran it will look for any:

- Usual warning that a mod can break any other mod's version
- What was found missing from the modpack
- Know when a content you downloaded was for the wrong loader version and change it to the same for your right loader *(while usually it does replace with the one for the same loader, it might not, in this case it will find the latest version for the correct loader)*


*and more! including obviously builing the modpack*
![](/.github/tapes/build.gif)


## Running

Running needs nothing, but it will most likely need libdbus to add the OS keychain when doing token saves, most builds will include it by default, but if it isn't run these in a terinal:

## On Ubuntu/Debian

```sh
sudo apt install libdbus-1-2
```

## On Fedora/RHEL

```sh
sudo dnf install dbus-libs
```

## On Arch

```sh
sudo pacman -S dbus
```

## On openSUSE

```sh
sudo zypper install libdbus-1-3
```

## On Alpine

```sh
sudo apk add dbus-libs
```

# Building

needs [rust](https://rust-lang.org/learn/get-started/) (1.85+) on your computer, on linux you need to install the keyring:

## On Ubuntu/Debian

```sh
sudo apt install libdbus-1-dev pkg-config
```

## On Fedora/RHEL

```sh
sudo dnf install dbus-devel pkgconf-pkg-config
```

## On Arch

```sh
sudo pacman -S dbus pkgconf
```

## On openSUSE

```sh
sudo zypper install dbus-1-devel pkg-config
```

## On Alpine

```sh
sudo apk add dbus-dev pkgconf
```