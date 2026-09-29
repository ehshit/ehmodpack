# __NAME__

Welcome to your modpack *(Built with eh's modpack)* here are the details:

| minecraft | loader | loader version | java |
| --- | --- | --- | --- |
__TARGET_ROWS__

## Adding Projects
- You can search for mods with `ehmodpack search`
- You can browse modrinth with `ehmodpack browse`

## Build it

If you have selected more then one, you can lock and build all of them at once

    ehmodpack lock
    ehmodpack build --locked --all

Or build a modpack for a loader and/or version:

    ehmodpack build --locked --mc 1.21.11 --loader fabric

Or if your project is for just a version:

    ehmodpack build

## Publishing it

You have 2 methods to publish this modpack to Modrinth:

### Using Workflows 

If your project is meant to be in a repository like GitHub, there will be automation for you as long as a Tag is created in the Releases tab, you only need to add secrets to it, which you can do by going to `Settings` --> `Secrets and variables` --> `Actions` --> and adding the following for `Repository secrets`:

- `{MODRINTH_TOKEN}` ([Modrinth Token](https://modrinth.com/settings/pats) with Create Versions and Write Versions)

And the one for the `Variables` tab under `Repository variables`

- `{MODRINTH_PROJECT}` Under your modpack project details --> 3 dots --> Copy ID

Or if you want to keep it closed to yourself and to your computer that's okay too, you can run:

    ehmodpack publish

Which will ask for your Token (usually in [Personal access tokens](https://modrinth.com/settings/pats)) and your Project ID (Under your modpack project details --> 3 dots --> Copy ID)

The files end on `/dist`, hope you have a nice day