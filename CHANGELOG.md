# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.4.2 - 2025-12-19

* Add poll_seconds, poll_millis & poll_micros attributes. These attributes could be used to declare
  a controller method that's run by the `run` method, periodically based on the timeout specified.

## 0.4.1 - 2025-11-26

* Allow specifying visibility on struct fields.

## 0.4.0 - 2025-11-26

### Breaking Changes

* Published field streams now yield the current value on first poll, then subsequent changes.
* Published field streams' item type is now the raw field type (e.g., `State`) instead of
  `*Changed` struct with `previous` and `new` fields.
* The `pub_setter` sub-attribute on `publish` has been removed. Use the new independent `setter`
  attribute instead (e.g., `#[controller(publish, setter)]`).

### New Features

* New `getter` attribute for fields: generates a client-side getter method. Supports custom naming
  via `#[controller(getter = "custom_name")]`.
* New `setter` attribute for fields: generates a client-side setter method independent of `publish`.
  Supports custom naming via `#[controller(setter = "custom_name")]`. Can be combined with `publish`
  to also broadcast changes.

## 0.3.0 - 2025-11-25

* Macro now operates on a module. This allows the macro to have a visibility on both the struct and
  the impl block and would enable us to improve the ergonomics of the API and add new API that's
  currently not possible due to the decoupling between the macro operating on the struct and impl
  block.
* Abstract the user from signal & state change types. Instead provide methods to create streams for
  receiving signals and state changes.
* Published fields can now have a client-side setter method if user asks for it through a new
  sub-attribute, `pub_setter`.
* A few minor fixes in documentation.

## 0.2.0 - 2025-11-17

* Update info in `Cargo.toml`.
* Port to latest embassy releases.
* Add missing changelog entry for 0.1.1.

## 0.1.1 - 2025-09-22

* Add repository link to `Cargo.toml`.

## 0.1.0 - 2024-10-11

First release. 🎉
