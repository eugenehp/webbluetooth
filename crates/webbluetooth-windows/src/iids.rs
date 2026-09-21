//! WinRT interface identifiers, vendored from Windows metadata.
//!
//! **Generated — do not edit.** Run `scripts/update.sh winrt-iids`.
//!
//! An IID is not derivable from a name and not worth transcribing by hand, so
//! these are extracted from `microsoft/windows-rs`, which is itself generated
//! from the Windows SDK metadata. The parameterised types in [`generics`] carry
//! the GUID of the *generic*, which [`crate::guid::Signature`] hashes together
//! with its arguments to get the real IID.

use crate::guid::Guid;

/// Interfaces, by their metadata name.
pub mod interfaces {
    use super::Guid;
    pub const DEFERRAL_COMPLETED_HANDLER: Guid =
        Guid::parse("ed32a372-f3c8-4faa-9cfb-470148da3888");
    pub const I_BLUETOOTH_ADAPTER: Guid = Guid::parse("7974f04c-5f7a-4a34-9225-a855f84b1a8b");
    pub const I_BLUETOOTH_ADAPTER2: Guid = Guid::parse("ac94cecc-24d5-41b3-916d-1097c50b102b");
    pub const I_BLUETOOTH_ADAPTER3: Guid = Guid::parse("8f8624e0-cba9-5211-9f89-3aac62b4c6b8");
    pub const I_BLUETOOTH_ADAPTER4: Guid = Guid::parse("f875f3e1-6d9a-5d5e-aee5-a17248e5f6dd");
    pub const I_BLUETOOTH_ADAPTER_STATICS: Guid =
        Guid::parse("8b02fb6a-ac4c-4741-8661-8eab7d17ea9f");
    pub const I_BLUETOOTH_CLASS_OF_DEVICE: Guid =
        Guid::parse("d640227e-d7d7-4661-9454-65039ca17a2b");
    pub const I_BLUETOOTH_CLASS_OF_DEVICE_STATICS: Guid =
        Guid::parse("e46135bd-0fa2-416c-91b4-c1e48ca061c1");
    pub const I_BLUETOOTH_DEVICE: Guid = Guid::parse("2335b156-90d2-4a04-aef5-0e20b9e6b707");
    pub const I_BLUETOOTH_DEVICE2: Guid = Guid::parse("0133f954-b156-4dd0-b1f5-c11bc31a5163");
    pub const I_BLUETOOTH_DEVICE3: Guid = Guid::parse("57fff78b-651a-4454-b90f-eb21ef0b0d71");
    pub const I_BLUETOOTH_DEVICE4: Guid = Guid::parse("817c34ad-0e9c-42b2-a8dc-3e8094940d12");
    pub const I_BLUETOOTH_DEVICE5: Guid = Guid::parse("b5e0b385-5e85-4559-a10d-1c7281379f96");
    pub const I_BLUETOOTH_DEVICE_ID: Guid = Guid::parse("c17949af-57c1-4642-bcce-e6c06b20ae76");
    pub const I_BLUETOOTH_DEVICE_ID_STATICS: Guid =
        Guid::parse("a7884e67-3efb-4f31-bbc2-810e09977404");
    pub const I_BLUETOOTH_DEVICE_STATICS: Guid =
        Guid::parse("0991df51-57db-4725-bbd7-84f64327ec2c");
    pub const I_BLUETOOTH_DEVICE_STATICS2: Guid =
        Guid::parse("c29e8e2f-4e14-4477-aa1b-b8b47e5b7ece");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT: Guid =
        Guid::parse("066fb2b7-33d1-4e7d-8367-cf81d0f79653");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_BYTE_PATTERN: Guid =
        Guid::parse("fbfad7f2-b9c5-4a08-bc51-502f8ef68a79");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_BYTE_PATTERN_FACTORY: Guid =
        Guid::parse("c2e24d73-fd5c-4ec3-be2a-9ca6fa11b7bd");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_DATA_SECTION: Guid =
        Guid::parse("d7213314-3a43-40f9-b6f0-92bfefc34ae3");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_DATA_SECTION_FACTORY: Guid =
        Guid::parse("e7a40942-a845-4045-bf7e-3e9971db8a6b");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_DATA_TYPES_STATICS: Guid =
        Guid::parse("3bb6472f-0606-434b-a76e-74159f0684d3");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_FILTER: Guid =
        Guid::parse("131eb0d3-d04e-47b1-837e-49405bf6f80f");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER: Guid =
        Guid::parse("cde820f9-d9fa-43d6-a264-ddd8b7da8b78");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER2: Guid =
        Guid::parse("fbdb545e-56f1-510f-a434-217fbd9e7bd2");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER3: Guid =
        Guid::parse("1cff3902-61ec-5776-ab86-9b41f94b1e66");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER_FACTORY: Guid =
        Guid::parse("5c5f065e-b863-4981-a1af-1c544d8b0c0d");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER_STATUS_CHANGED_EVENT_ARGS: Guid =
        Guid::parse("09c2bd9f-2dff-4b23-86ee-0d14fb94aeae");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER_STATUS_CHANGED_EVENT_ARGS2: Guid =
        Guid::parse("8f62790e-dc88-5c8b-b34e-10b321850f88");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_RECEIVED_EVENT_ARGS: Guid =
        Guid::parse("27987ddf-e596-41be-8d43-9e6731d4a913");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_RECEIVED_EVENT_ARGS2: Guid =
        Guid::parse("12d9c87b-0399-5f0e-a348-53b02b6b162e");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_RECEIVED_EVENT_ARGS3: Guid =
        Guid::parse("8d204b54-ff86-5d84-a25a-137dccd96f7a");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_SCAN_PARAMETERS: Guid =
        Guid::parse("94f91413-63d9-53bd-af4c-e6b1a6514595");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_SCAN_PARAMETERS_STATICS: Guid =
        Guid::parse("548e39cd-3c9e-5f8d-b5e1-adebed5c357c");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_WATCHER: Guid =
        Guid::parse("a6ac336f-f3d3-4297-8d6c-c81ea6623f40");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_WATCHER2: Guid =
        Guid::parse("01bf26bc-b164-5805-90a3-e8a7997ff225");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_WATCHER3: Guid =
        Guid::parse("14d980be-4002-5dbe-8519-ffca6ca389f0");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_WATCHER_FACTORY: Guid =
        Guid::parse("9aaf2d56-39ac-453e-b32a-85c657e017f1");
    pub const I_BLUETOOTH_LE_ADVERTISEMENT_WATCHER_STOPPED_EVENT_ARGS: Guid =
        Guid::parse("dd40f84d-e7b9-43e3-9c04-0685d085fd8c");
    pub const I_BLUETOOTH_LE_APPEARANCE: Guid = Guid::parse("5d2079f2-66a8-4258-985e-02b4d9509f18");
    pub const I_BLUETOOTH_LE_APPEARANCE_CATEGORIES_STATICS: Guid =
        Guid::parse("6d4d54fe-046a-4185-aab6-824cf0610861");
    pub const I_BLUETOOTH_LE_APPEARANCE_STATICS: Guid =
        Guid::parse("a193c0c7-4504-4f4a-9ba5-cd1054e5e065");
    pub const I_BLUETOOTH_LE_APPEARANCE_SUBCATEGORIES_STATICS: Guid =
        Guid::parse("e57ba606-2144-415a-8312-71ccf291f8d1");
    pub const I_BLUETOOTH_LE_CONNECTION_PARAMETERS: Guid =
        Guid::parse("33cb0771-8da9-508f-a366-1ca388c929ab");
    pub const I_BLUETOOTH_LE_CONNECTION_PHY: Guid =
        Guid::parse("781e5e48-621e-5a7e-8be6-1b9561ff63c9");
    pub const I_BLUETOOTH_LE_CONNECTION_PHY_INFO: Guid =
        Guid::parse("9a100bdd-602e-5c27-a1ae-b230015a6394");
    pub const I_BLUETOOTH_LE_DEVICE: Guid = Guid::parse("b5ee2f7b-4ad8-4642-ac48-80a0b500e887");
    pub const I_BLUETOOTH_LE_DEVICE2: Guid = Guid::parse("26f062b3-7aee-4d31-baba-b1b9775f5916");
    pub const I_BLUETOOTH_LE_DEVICE3: Guid = Guid::parse("aee9e493-44ac-40dc-af33-b2c13c01ca46");
    pub const I_BLUETOOTH_LE_DEVICE4: Guid = Guid::parse("2b605031-2248-4b2f-acf0-7cee36fc5870");
    pub const I_BLUETOOTH_LE_DEVICE5: Guid = Guid::parse("9d6a1260-5287-458e-95ba-17c8b7bb326e");
    pub const I_BLUETOOTH_LE_DEVICE6: Guid = Guid::parse("ca7190ef-0cae-573c-a1ca-e1fc5bfc39e2");
    pub const I_BLUETOOTH_LE_DEVICE_STATICS: Guid =
        Guid::parse("c8cf1a19-f0b6-4bf0-8689-41303de2d9f4");
    pub const I_BLUETOOTH_LE_DEVICE_STATICS2: Guid =
        Guid::parse("5f12c06b-3bac-43e8-ad16-563271bd41c2");
    pub const I_BLUETOOTH_LE_MANUFACTURER_DATA: Guid =
        Guid::parse("912dba18-6963-4533-b061-4694dafb34e5");
    pub const I_BLUETOOTH_LE_MANUFACTURER_DATA_FACTORY: Guid =
        Guid::parse("c09b39f8-319a-441e-8de5-66a81e877a6c");
    pub const I_BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS: Guid =
        Guid::parse("f2f44344-7372-5f7b-9b34-29c944f5a715");
    pub const I_BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS_REQUEST: Guid =
        Guid::parse("8a375276-a528-5266-b661-cce6a5ff9739");
    pub const I_BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS_STATICS: Guid =
        Guid::parse("0e3e8edc-2751-55aa-a838-8faeee818d72");
    pub const I_BLUETOOTH_SIGNAL_STRENGTH_FILTER: Guid =
        Guid::parse("df7b7391-6bb5-4cfe-90b1-5d7324edcf7f");
    pub const I_BLUETOOTH_UUID_HELPER_STATICS: Guid =
        Guid::parse("17df0cd8-cf74-4b21-afe6-f57a11bcdea0");
    pub const I_BUFFER: Guid = Guid::parse("905a0fe0-bc53-11df-8c49-001e4fc686da");
    pub const I_BUFFER_FACTORY: Guid = Guid::parse("71af914d-c10f-484b-bc50-14bc623b3a27");
    pub const I_BUFFER_STATICS: Guid = Guid::parse("e901e65b-d716-475a-a90a-af7229b1e741");
    pub const I_CLOSABLE: Guid = Guid::parse("30d5a829-7fa4-4026-83bb-d75bae4ea99e");
    pub const I_CONTENT_TYPE_PROVIDER: Guid = Guid::parse("97d098a5-3b99-4de9-88a5-e11d2f50c795");
    pub const I_DATA_READER: Guid = Guid::parse("e2b50029-b4c1-4314-a4b8-fb813a2f275e");
    pub const I_DATA_READER_FACTORY: Guid = Guid::parse("d7527847-57da-4e15-914c-06806699a098");
    pub const I_DATA_READER_STATICS: Guid = Guid::parse("11fcbfc8-f93a-471b-b121-f379e349313c");
    pub const I_DATA_WRITER: Guid = Guid::parse("64b89265-d341-4922-b38a-dd4af8808c4e");
    pub const I_DATA_WRITER_FACTORY: Guid = Guid::parse("338c67c2-8b84-4c2b-9c50-7b8767847a1f");
    pub const I_DEFERRAL: Guid = Guid::parse("d6269732-3b7f-46a7-b40b-4fdca2a2c693");
    pub const I_DEFERRAL_FACTORY: Guid = Guid::parse("65a1ecc5-3fb5-4832-8ca9-f061b281d13a");
    pub const I_DEVICE_ACCESS_CHANGED_EVENT_ARGS: Guid =
        Guid::parse("deda0bcc-4f9d-4f58-9dba-a9bc800408d5");
    pub const I_DEVICE_ACCESS_CHANGED_EVENT_ARGS2: Guid =
        Guid::parse("82523262-934b-4b30-a178-adc39f2f2be3");
    pub const I_DEVICE_ACCESS_CHANGED_EVENT_ARGS3: Guid =
        Guid::parse("7580a878-7fd9-5cd7-8560-3c644b9b10db");
    pub const I_DEVICE_ACCESS_INFORMATION: Guid =
        Guid::parse("0baa9a73-6de5-4915-8ddd-9a0554a6f545");
    pub const I_DEVICE_ACCESS_INFORMATION2: Guid =
        Guid::parse("e2b9dff6-e88f-5d0a-9c1e-d788808df47b");
    pub const I_DEVICE_ACCESS_INFORMATION_STATICS: Guid =
        Guid::parse("574bd3d3-5f30-45cd-8a94-724fe5973084");
    pub const I_DEVICE_CONNECTION_CHANGE_TRIGGER_DETAILS: Guid =
        Guid::parse("b8578c0c-bbc1-484b-bffa-7b31dcc200b2");
    pub const I_DEVICE_DISCONNECT_BUTTON_CLICKED_EVENT_ARGS: Guid =
        Guid::parse("8e44b56d-f902-4a00-b536-f37992e6a2a7");
    pub const I_DEVICE_ENUMERATION_SETTINGS: Guid =
        Guid::parse("f7710f66-9ff3-41c8-85eb-87f81148a30f");
    pub const I_DEVICE_INFORMATION: Guid = Guid::parse("aba0fb95-4398-489d-8e44-e6130927011f");
    pub const I_DEVICE_INFORMATION2: Guid = Guid::parse("f156a638-7997-48d9-a10c-269d46533f48");
    pub const I_DEVICE_INFORMATION_CUSTOM_PAIRING: Guid =
        Guid::parse("85138c02-4ee6-4914-8370-107a39144c0e");
    pub const I_DEVICE_INFORMATION_CUSTOM_PAIRING2: Guid =
        Guid::parse("0ebda662-e696-5fa9-8f72-80cfebcd851d");
    pub const I_DEVICE_INFORMATION_PAIRING: Guid =
        Guid::parse("2c4769f5-f684-40d5-8469-e8dbaab70485");
    pub const I_DEVICE_INFORMATION_PAIRING2: Guid =
        Guid::parse("f68612fd-0aee-4328-85cc-1c742bb1790d");
    pub const I_DEVICE_INFORMATION_PAIRING_STATICS: Guid =
        Guid::parse("e915c408-36d4-49a1-bf13-514173799b6b");
    pub const I_DEVICE_INFORMATION_PAIRING_STATICS2: Guid =
        Guid::parse("04de5372-b7b7-476b-a74f-c5836a704d98");
    pub const I_DEVICE_INFORMATION_STATICS: Guid =
        Guid::parse("c17f100e-3a46-4a78-8013-769dc9b97390");
    pub const I_DEVICE_INFORMATION_STATICS2: Guid =
        Guid::parse("493b4f34-a84f-45fd-9167-15d1cb1bd1f9");
    pub const I_DEVICE_INFORMATION_STATICS3: Guid =
        Guid::parse("25f06279-9364-5a6c-8a54-5d4a6d3d922a");
    pub const I_DEVICE_INFORMATION_UPDATE: Guid =
        Guid::parse("8f315305-d972-44b7-a37e-9e822c78213b");
    pub const I_DEVICE_INFORMATION_UPDATE2: Guid =
        Guid::parse("5d9d148c-a873-485e-baa6-aa620788e3cc");
    pub const I_DEVICE_PAIRING_REQUESTED_EVENT_ARGS: Guid =
        Guid::parse("f717fc56-de6b-487f-8376-0180aca69963");
    pub const I_DEVICE_PAIRING_REQUESTED_EVENT_ARGS2: Guid =
        Guid::parse("c83752d9-e4d3-4db0-a360-a105e437dbdc");
    pub const I_DEVICE_PAIRING_REQUESTED_EVENT_ARGS3: Guid =
        Guid::parse("195e5a38-43dc-562f-babe-efc8b110088b");
    pub const I_DEVICE_PAIRING_RESULT: Guid = Guid::parse("072b02bf-dd95-4025-9b37-de51adba37b7");
    pub const I_DEVICE_PAIRING_SET_MEMBERS_REQUESTED_EVENT_ARGS: Guid =
        Guid::parse("7fb42cff-ecac-5012-8d7d-a1894680a349");
    pub const I_DEVICE_PAIRING_SETTINGS: Guid = Guid::parse("482cb27c-83bb-420e-be51-6602b222de54");
    pub const I_DEVICE_PICKER: Guid = Guid::parse("84997aa2-034a-4440-8813-7d0bd479bf5a");
    pub const I_DEVICE_PICKER_APPEARANCE: Guid =
        Guid::parse("e69a12c6-e627-4ed8-9b6c-460af445e56d");
    pub const I_DEVICE_PICKER_FILTER: Guid = Guid::parse("91db92a2-57cb-48f1-9b59-a59b7a1f02a2");
    pub const I_DEVICE_SELECTED_EVENT_ARGS: Guid =
        Guid::parse("269edade-1d2f-4940-8402-4156b81d3c77");
    pub const I_DEVICE_UNPAIRING_RESULT: Guid = Guid::parse("66f44ad3-79d9-444b-92cf-a92ef72571c7");
    pub const I_DEVICE_WATCHER: Guid = Guid::parse("c9eab97d-8f6b-4f96-a9f4-abc814e22271");
    pub const I_DEVICE_WATCHER2: Guid = Guid::parse("ff08456e-ed14-49e9-9a69-8117c54ae971");
    pub const I_DEVICE_WATCHER_EVENT: Guid = Guid::parse("74aa9c0b-1dbd-47fd-b635-3cc556d0ff8b");
    pub const I_DEVICE_WATCHER_TRIGGER_DETAILS: Guid =
        Guid::parse("38808119-4cb7-4e57-a56d-776d07cbfef9");
    pub const I_ENCLOSURE_LOCATION: Guid = Guid::parse("42340a27-5810-459c-aabb-c65e1f813ecf");
    pub const I_ENCLOSURE_LOCATION2: Guid = Guid::parse("2885995b-e07d-485d-8a9e-bdf29aef4f66");
    pub const I_FILE_RANDOM_ACCESS_STREAM_STATICS: Guid =
        Guid::parse("73550107-3b57-4b5d-8345-554d2fc621f0");
    pub const I_GATT_CHARACTERISTIC: Guid = Guid::parse("59cb50c1-5934-4f68-a198-eb864fa44e6b");
    pub const I_GATT_CHARACTERISTIC2: Guid = Guid::parse("ae1ab578-ec06-4764-b780-9835a1d35d6e");
    pub const I_GATT_CHARACTERISTIC3: Guid = Guid::parse("3f3c663e-93d4-406b-b817-db81f8ed53b3");
    pub const I_GATT_CHARACTERISTIC_STATICS: Guid =
        Guid::parse("59cb50c3-5934-4f68-a198-eb864fa44e6b");
    pub const I_GATT_CHARACTERISTIC_UUIDS_STATICS: Guid =
        Guid::parse("58fa4586-b1de-470c-b7de-0d11ff44f4b7");
    pub const I_GATT_CHARACTERISTIC_UUIDS_STATICS2: Guid =
        Guid::parse("1855b425-d46e-4a2c-9c3f-ed6dea29e7be");
    pub const I_GATT_CHARACTERISTICS_RESULT: Guid =
        Guid::parse("1194945c-b257-4f3e-9db7-f68bc9a9aef2");
    pub const I_GATT_CLIENT_NOTIFICATION_RESULT: Guid =
        Guid::parse("506d5599-0112-419a-8e3b-ae21afabd2c2");
    pub const I_GATT_CLIENT_NOTIFICATION_RESULT2: Guid =
        Guid::parse("8faec497-45e0-497e-9582-29a1fe281ad5");
    pub const I_GATT_DESCRIPTOR: Guid = Guid::parse("92055f2b-8084-4344-b4c2-284de19a8506");
    pub const I_GATT_DESCRIPTOR2: Guid = Guid::parse("8f563d39-d630-406c-ba11-10cdd16b0e5e");
    pub const I_GATT_DESCRIPTOR_STATICS: Guid = Guid::parse("92055f2d-8084-4344-b4c2-284de19a8506");
    pub const I_GATT_DESCRIPTOR_UUIDS_STATICS: Guid =
        Guid::parse("a6f862ce-9cfc-42f1-9185-ff37b75181d3");
    pub const I_GATT_DESCRIPTORS_RESULT: Guid = Guid::parse("9bc091f3-95e7-4489-8d25-ff81955a57b9");
    pub const I_GATT_DEVICE_SERVICE: Guid = Guid::parse("ac7b7c05-b33c-47cf-990f-6b8f5577df71");
    pub const I_GATT_DEVICE_SERVICE2: Guid = Guid::parse("fc54520b-0b0d-4708-bae0-9ffd9489bc59");
    pub const I_GATT_DEVICE_SERVICE3: Guid = Guid::parse("b293a950-0c53-437c-a9b3-5c3210c6e569");
    pub const I_GATT_DEVICE_SERVICE_STATICS: Guid =
        Guid::parse("196d0022-faad-45dc-ae5b-2ac3184e84db");
    pub const I_GATT_DEVICE_SERVICE_STATICS2: Guid =
        Guid::parse("0604186e-24a6-4b0d-a2f2-30cc01545d25");
    pub const I_GATT_DEVICE_SERVICES_RESULT: Guid =
        Guid::parse("171dd3ee-016d-419d-838a-576cf475a3d8");
    pub const I_GATT_LOCAL_CHARACTERISTIC: Guid =
        Guid::parse("aede376d-5412-4d74-92a8-8deb8526829c");
    pub const I_GATT_LOCAL_CHARACTERISTIC_PARAMETERS: Guid =
        Guid::parse("faf73db4-4cff-44c7-8445-040e6ead0063");
    pub const I_GATT_LOCAL_CHARACTERISTIC_RESULT: Guid =
        Guid::parse("7975de9b-0170-4397-9666-92f863f12ee6");
    pub const I_GATT_LOCAL_DESCRIPTOR: Guid = Guid::parse("f48ebe06-789d-4a4b-8652-bd017b5d2fc6");
    pub const I_GATT_LOCAL_DESCRIPTOR_PARAMETERS: Guid =
        Guid::parse("5fdede6a-f3c1-4b66-8c4b-e3d2293b40e9");
    pub const I_GATT_LOCAL_DESCRIPTOR_RESULT: Guid =
        Guid::parse("375791be-321f-4366-bfc1-3bc6b82c79f8");
    pub const I_GATT_LOCAL_SERVICE: Guid = Guid::parse("f513e258-f7f7-4902-b803-57fcc7d6fe83");
    pub const I_GATT_PRESENTATION_FORMAT: Guid =
        Guid::parse("196d0021-faad-45dc-ae5b-2ac3184e84db");
    pub const I_GATT_PRESENTATION_FORMAT_STATICS: Guid =
        Guid::parse("196d0020-faad-45dc-ae5b-2ac3184e84db");
    pub const I_GATT_PRESENTATION_FORMAT_STATICS2: Guid =
        Guid::parse("a9c21713-b82f-435e-b634-21fd85a43c07");
    pub const I_GATT_PRESENTATION_FORMAT_TYPES_STATICS: Guid =
        Guid::parse("faf1ba0a-30ba-409c-bef7-cffb6d03b8fb");
    pub const I_GATT_PROTOCOL_ERROR_STATICS: Guid =
        Guid::parse("ca46c5c5-0ecc-4809-bea3-cf79bc991e37");
    pub const I_GATT_READ_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_RESULT: Guid =
        Guid::parse("63a66f09-1aea-4c4c-a50f-97bae474b348");
    pub const I_GATT_READ_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_RESULT2: Guid =
        Guid::parse("1bf1a59d-ba4d-4622-8651-f4ee150d0a5d");
    pub const I_GATT_READ_REQUEST: Guid = Guid::parse("f1dd6535-6acd-42a6-a4bb-d789dae0043e");
    pub const I_GATT_READ_REQUESTED_EVENT_ARGS: Guid =
        Guid::parse("93497243-f39c-484b-8ab6-996ba486cfa3");
    pub const I_GATT_READ_RESULT: Guid = Guid::parse("63a66f08-1aea-4c4c-a50f-97bae474b348");
    pub const I_GATT_READ_RESULT2: Guid = Guid::parse("a10f50a0-fb43-48af-baaa-638a5c6329fe");
    pub const I_GATT_RELIABLE_WRITE_TRANSACTION: Guid =
        Guid::parse("63a66f07-1aea-4c4c-a50f-97bae474b348");
    pub const I_GATT_RELIABLE_WRITE_TRANSACTION2: Guid =
        Guid::parse("51113987-ef12-462f-9fb2-a1a43a679416");
    pub const I_GATT_REQUEST_STATE_CHANGED_EVENT_ARGS: Guid =
        Guid::parse("e834d92c-27be-44b3-9d0d-4fc6e808dd3f");
    pub const I_GATT_SERVICE_PROVIDER: Guid = Guid::parse("7822b3cd-2889-4f86-a051-3f0aed1c2760");
    pub const I_GATT_SERVICE_PROVIDER2: Guid = Guid::parse("9ef531a9-cf12-59a3-a81c-362f4aabaacf");
    pub const I_GATT_SERVICE_PROVIDER_ADVERTISEMENT_STATUS_CHANGED_EVENT_ARGS: Guid =
        Guid::parse("59a5aa65-fa21-4ffc-b155-04d928012686");
    pub const I_GATT_SERVICE_PROVIDER_ADVERTISING_PARAMETERS: Guid =
        Guid::parse("e2ce31ab-6315-4c22-9bd7-781dbc3d8d82");
    pub const I_GATT_SERVICE_PROVIDER_ADVERTISING_PARAMETERS2: Guid =
        Guid::parse("ff68468d-ca92-4434-9743-0e90988ad879");
    pub const I_GATT_SERVICE_PROVIDER_ADVERTISING_PARAMETERS3: Guid =
        Guid::parse("a23546b2-b216-5929-9055-f1313dd53e2a");
    pub const I_GATT_SERVICE_PROVIDER_RESULT: Guid =
        Guid::parse("764696d8-c53e-428c-8a48-67afe02c3ae6");
    pub const I_GATT_SERVICE_PROVIDER_STATICS: Guid =
        Guid::parse("31794063-5256-4054-a4f4-7bbe7755a57e");
    pub const I_GATT_SERVICE_UUIDS_STATICS: Guid =
        Guid::parse("6dc57058-9aba-4417-b8f2-dce016d34ee2");
    pub const I_GATT_SERVICE_UUIDS_STATICS2: Guid =
        Guid::parse("d2ae94f5-3d15-4f79-9c0c-eaafa675155c");
    pub const I_GATT_SESSION: Guid = Guid::parse("d23b5143-e04e-4c24-999c-9c256f9856b1");
    pub const I_GATT_SESSION_STATICS: Guid = Guid::parse("2e65b95c-539f-4db7-82a8-73bdbbf73ebf");
    pub const I_GATT_SESSION_STATUS_CHANGED_EVENT_ARGS: Guid =
        Guid::parse("7605b72e-837f-404c-ab34-3163f39ddf32");
    pub const I_GATT_SUBSCRIBED_CLIENT: Guid = Guid::parse("736e9001-15a4-4ec2-9248-e3f20d463be9");
    pub const I_GATT_VALUE_CHANGED_EVENT_ARGS: Guid =
        Guid::parse("d21bdb54-06e3-4ed8-a263-acfac8ba7313");
    pub const I_GATT_WRITE_REQUEST: Guid = Guid::parse("aeb6a9ed-de2f-4fc2-a9a8-94ea7844f13d");
    pub const I_GATT_WRITE_REQUESTED_EVENT_ARGS: Guid =
        Guid::parse("2dec8bbe-a73a-471a-94d5-037deadd0806");
    pub const I_GATT_WRITE_RESULT: Guid = Guid::parse("4991ddb1-cb2b-44f7-99fc-d29a2871dc9b");
    pub const I_GET_ACTIVATION_FACTORY: Guid = Guid::parse("4edb8ee2-96dd-49a7-94f7-4607ddab8e3c");
    pub const I_GUID_HELPER_STATICS: Guid = Guid::parse("59c7966b-ae52-5283-ad7f-a1b9e9678add");
    pub const I_INPUT_STREAM: Guid = Guid::parse("905a0fe2-bc53-11df-8c49-001e4fc686da");
    pub const I_INPUT_STREAM_REFERENCE: Guid = Guid::parse("43929d18-5ec9-4b5a-919c-4205b0c804b6");
    pub const I_MEMORY_BUFFER: Guid = Guid::parse("fbc4dd2a-245b-11e4-af98-689423260cf8");
    pub const I_MEMORY_BUFFER_FACTORY: Guid = Guid::parse("fbc4dd2b-245b-11e4-af98-689423260cf8");
    pub const I_MEMORY_BUFFER_REFERENCE: Guid = Guid::parse("fbc4dd29-245b-11e4-af98-689423260cf8");
    pub const I_OUTPUT_STREAM: Guid = Guid::parse("905a0fe6-bc53-11df-8c49-001e4fc686da");
    pub const I_PROPERTY_SET_SERIALIZER: Guid = Guid::parse("6e8ebf1c-ef3d-4376-b20e-5be638aeac77");
    pub const I_PROPERTY_VALUE: Guid = Guid::parse("4bd682dd-7554-40e9-9a9b-82654ede7e62");
    pub const I_PROPERTY_VALUE_STATICS: Guid = Guid::parse("629bdbc8-d932-4ff4-96b9-8d96c5c1e858");
    pub const I_RANDOM_ACCESS_STREAM: Guid = Guid::parse("905a0fe1-bc53-11df-8c49-001e4fc686da");
    pub const I_RANDOM_ACCESS_STREAM_REFERENCE: Guid =
        Guid::parse("33ee3134-1dd6-4e3a-8067-d1c162e8642b");
    pub const I_RANDOM_ACCESS_STREAM_REFERENCE_STATICS: Guid =
        Guid::parse("857309dc-3fbf-4e7d-986f-ef3b1a07a964");
    pub const I_RANDOM_ACCESS_STREAM_STATICS: Guid =
        Guid::parse("524cedcf-6e29-4ce5-9573-6b753db66c3a");
    pub const I_RANDOM_ACCESS_STREAM_WITH_CONTENT_TYPE: Guid =
        Guid::parse("cc254827-4b3d-438f-9232-10c76bc7e038");
    pub const I_STRINGABLE: Guid = Guid::parse("96369f54-8eb6-48f0-abce-c1b211e627c3");
    pub const I_URI_ESCAPE_STATICS: Guid = Guid::parse("c1d432ba-c824-4452-a7fd-512bc3bbe9a1");
    pub const I_URI_RUNTIME_CLASS: Guid = Guid::parse("9e365e57-48b2-4160-956f-c7385120bbfc");
    pub const I_URI_RUNTIME_CLASS_FACTORY: Guid =
        Guid::parse("44a9796f-723e-4fdf-a218-033e75b0c084");
    pub const I_URI_RUNTIME_CLASS_WITH_ABSOLUTE_CANONICAL_URI: Guid =
        Guid::parse("758d9661-221c-480f-a339-50656673f46f");
    pub const I_WWW_FORM_URL_DECODER_ENTRY: Guid =
        Guid::parse("125e7431-f678-4e8e-b670-20a9b06c512d");
    pub const I_WWW_FORM_URL_DECODER_RUNTIME_CLASS: Guid =
        Guid::parse("d45a0451-f225-4542-9296-0e1df5d254df");
    pub const I_WWW_FORM_URL_DECODER_RUNTIME_CLASS_FACTORY: Guid =
        Guid::parse("5b8c6b3d-24ae-41b5-a1bf-f0c3d544845b");
}

/// Runtime classes: their full name, and the default interface a signature
/// string names alongside it.
pub mod classes {
    use super::Guid;

    /// `(full name, default interface)` — everything `rc(...)` needs.
    pub type Class = (&'static str, Guid);

    pub const BLUETOOTH_ADAPTER: Class = (
        "Windows.Devices.Bluetooth.BluetoothAdapter",
        Guid::parse("7974f04c-5f7a-4a34-9225-a855f84b1a8b"),
    );
    pub const BLUETOOTH_CLASS_OF_DEVICE: Class = (
        "Windows.Devices.Bluetooth.BluetoothClassOfDevice",
        Guid::parse("d640227e-d7d7-4661-9454-65039ca17a2b"),
    );
    pub const BLUETOOTH_DEVICE: Class = (
        "Windows.Devices.Bluetooth.BluetoothDevice",
        Guid::parse("2335b156-90d2-4a04-aef5-0e20b9e6b707"),
    );
    pub const BLUETOOTH_DEVICE_ID: Class = (
        "Windows.Devices.Bluetooth.BluetoothDeviceId",
        Guid::parse("c17949af-57c1-4642-bcce-e6c06b20ae76"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisement",
        Guid::parse("066fb2b7-33d1-4e7d-8367-cf81d0f79653"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_BYTE_PATTERN: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementBytePattern",
        Guid::parse("fbfad7f2-b9c5-4a08-bc51-502f8ef68a79"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_DATA_SECTION: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementDataSection",
        Guid::parse("d7213314-3a43-40f9-b6f0-92bfefc34ae3"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_FILTER: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementFilter",
        Guid::parse("131eb0d3-d04e-47b1-837e-49405bf6f80f"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementPublisher",
        Guid::parse("cde820f9-d9fa-43d6-a264-ddd8b7da8b78"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_PUBLISHER_STATUS_CHANGED_EVENT_ARGS: Class = ("Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementPublisherStatusChangedEventArgs", Guid::parse("09c2bd9f-2dff-4b23-86ee-0d14fb94aeae"));
    pub const BLUETOOTH_LE_ADVERTISEMENT_RECEIVED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementReceivedEventArgs",
        Guid::parse("27987ddf-e596-41be-8d43-9e6731d4a913"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_SCAN_PARAMETERS: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementScanParameters",
        Guid::parse("94f91413-63d9-53bd-af4c-e6b1a6514595"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_WATCHER: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementWatcher",
        Guid::parse("a6ac336f-f3d3-4297-8d6c-c81ea6623f40"),
    );
    pub const BLUETOOTH_LE_ADVERTISEMENT_WATCHER_STOPPED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEAdvertisementWatcherStoppedEventArgs",
        Guid::parse("dd40f84d-e7b9-43e3-9c04-0685d085fd8c"),
    );
    pub const BLUETOOTH_LE_APPEARANCE: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEAppearance",
        Guid::parse("5d2079f2-66a8-4258-985e-02b4d9509f18"),
    );
    pub const BLUETOOTH_LE_CONNECTION_PARAMETERS: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEConnectionParameters",
        Guid::parse("33cb0771-8da9-508f-a366-1ca388c929ab"),
    );
    pub const BLUETOOTH_LE_CONNECTION_PHY: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEConnectionPhy",
        Guid::parse("781e5e48-621e-5a7e-8be6-1b9561ff63c9"),
    );
    pub const BLUETOOTH_LE_CONNECTION_PHY_INFO: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEConnectionPhyInfo",
        Guid::parse("9a100bdd-602e-5c27-a1ae-b230015a6394"),
    );
    pub const BLUETOOTH_LE_DEVICE: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEDevice",
        Guid::parse("b5ee2f7b-4ad8-4642-ac48-80a0b500e887"),
    );
    pub const BLUETOOTH_LE_MANUFACTURER_DATA: Class = (
        "Windows.Devices.Bluetooth.Advertisement.BluetoothLEManufacturerData",
        Guid::parse("912dba18-6963-4533-b061-4694dafb34e5"),
    );
    pub const BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEPreferredConnectionParameters",
        Guid::parse("f2f44344-7372-5f7b-9b34-29c944f5a715"),
    );
    pub const BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS_REQUEST: Class = (
        "Windows.Devices.Bluetooth.BluetoothLEPreferredConnectionParametersRequest",
        Guid::parse("8a375276-a528-5266-b661-cce6a5ff9739"),
    );
    pub const BLUETOOTH_SIGNAL_STRENGTH_FILTER: Class = (
        "Windows.Devices.Bluetooth.BluetoothSignalStrengthFilter",
        Guid::parse("df7b7391-6bb5-4cfe-90b1-5d7324edcf7f"),
    );
    pub const BUFFER: Class = (
        "Windows.Storage.Streams.Buffer",
        Guid::parse("905a0fe0-bc53-11df-8c49-001e4fc686da"),
    );
    pub const DATA_READER: Class = (
        "Windows.Storage.Streams.DataReader",
        Guid::parse("e2b50029-b4c1-4314-a4b8-fb813a2f275e"),
    );
    pub const DATA_WRITER: Class = (
        "Windows.Storage.Streams.DataWriter",
        Guid::parse("64b89265-d341-4922-b38a-dd4af8808c4e"),
    );
    pub const DEFERRAL: Class = (
        "Windows.Foundation.Deferral",
        Guid::parse("d6269732-3b7f-46a7-b40b-4fdca2a2c693"),
    );
    pub const DEVICE_ACCESS_CHANGED_EVENT_ARGS: Class = (
        "Windows.Devices.Enumeration.DeviceAccessChangedEventArgs",
        Guid::parse("deda0bcc-4f9d-4f58-9dba-a9bc800408d5"),
    );
    pub const DEVICE_ACCESS_INFORMATION: Class = (
        "Windows.Devices.Enumeration.DeviceAccessInformation",
        Guid::parse("0baa9a73-6de5-4915-8ddd-9a0554a6f545"),
    );
    pub const DEVICE_CONNECTION_CHANGE_TRIGGER_DETAILS: Class = (
        "Windows.Devices.Enumeration.DeviceConnectionChangeTriggerDetails",
        Guid::parse("b8578c0c-bbc1-484b-bffa-7b31dcc200b2"),
    );
    pub const DEVICE_DISCONNECT_BUTTON_CLICKED_EVENT_ARGS: Class = (
        "Windows.Devices.Enumeration.DeviceDisconnectButtonClickedEventArgs",
        Guid::parse("8e44b56d-f902-4a00-b536-f37992e6a2a7"),
    );
    pub const DEVICE_INFORMATION: Class = (
        "Windows.Devices.Enumeration.DeviceInformation",
        Guid::parse("aba0fb95-4398-489d-8e44-e6130927011f"),
    );
    pub const DEVICE_INFORMATION_CUSTOM_PAIRING: Class = (
        "Windows.Devices.Enumeration.DeviceInformationCustomPairing",
        Guid::parse("85138c02-4ee6-4914-8370-107a39144c0e"),
    );
    pub const DEVICE_INFORMATION_PAIRING: Class = (
        "Windows.Devices.Enumeration.DeviceInformationPairing",
        Guid::parse("2c4769f5-f684-40d5-8469-e8dbaab70485"),
    );
    pub const DEVICE_INFORMATION_UPDATE: Class = (
        "Windows.Devices.Enumeration.DeviceInformationUpdate",
        Guid::parse("8f315305-d972-44b7-a37e-9e822c78213b"),
    );
    pub const DEVICE_PAIRING_REQUESTED_EVENT_ARGS: Class = (
        "Windows.Devices.Enumeration.DevicePairingRequestedEventArgs",
        Guid::parse("f717fc56-de6b-487f-8376-0180aca69963"),
    );
    pub const DEVICE_PAIRING_RESULT: Class = (
        "Windows.Devices.Enumeration.DevicePairingResult",
        Guid::parse("072b02bf-dd95-4025-9b37-de51adba37b7"),
    );
    pub const DEVICE_PAIRING_SET_MEMBERS_REQUESTED_EVENT_ARGS: Class = (
        "Windows.Devices.Enumeration.DevicePairingSetMembersRequestedEventArgs",
        Guid::parse("7fb42cff-ecac-5012-8d7d-a1894680a349"),
    );
    pub const DEVICE_PICKER: Class = (
        "Windows.Devices.Enumeration.DevicePicker",
        Guid::parse("84997aa2-034a-4440-8813-7d0bd479bf5a"),
    );
    pub const DEVICE_PICKER_APPEARANCE: Class = (
        "Windows.Devices.Enumeration.DevicePickerAppearance",
        Guid::parse("e69a12c6-e627-4ed8-9b6c-460af445e56d"),
    );
    pub const DEVICE_PICKER_FILTER: Class = (
        "Windows.Devices.Enumeration.DevicePickerFilter",
        Guid::parse("91db92a2-57cb-48f1-9b59-a59b7a1f02a2"),
    );
    pub const DEVICE_SELECTED_EVENT_ARGS: Class = (
        "Windows.Devices.Enumeration.DeviceSelectedEventArgs",
        Guid::parse("269edade-1d2f-4940-8402-4156b81d3c77"),
    );
    pub const DEVICE_UNPAIRING_RESULT: Class = (
        "Windows.Devices.Enumeration.DeviceUnpairingResult",
        Guid::parse("66f44ad3-79d9-444b-92cf-a92ef72571c7"),
    );
    pub const DEVICE_WATCHER: Class = (
        "Windows.Devices.Enumeration.DeviceWatcher",
        Guid::parse("c9eab97d-8f6b-4f96-a9f4-abc814e22271"),
    );
    pub const DEVICE_WATCHER_EVENT: Class = (
        "Windows.Devices.Enumeration.DeviceWatcherEvent",
        Guid::parse("74aa9c0b-1dbd-47fd-b635-3cc556d0ff8b"),
    );
    pub const DEVICE_WATCHER_TRIGGER_DETAILS: Class = (
        "Windows.Devices.Enumeration.DeviceWatcherTriggerDetails",
        Guid::parse("38808119-4cb7-4e57-a56d-776d07cbfef9"),
    );
    pub const ENCLOSURE_LOCATION: Class = (
        "Windows.Devices.Enumeration.EnclosureLocation",
        Guid::parse("42340a27-5810-459c-aabb-c65e1f813ecf"),
    );
    pub const FILE_INPUT_STREAM: Class = (
        "Windows.Storage.Streams.FileInputStream",
        Guid::parse("905a0fe2-bc53-11df-8c49-001e4fc686da"),
    );
    pub const FILE_OUTPUT_STREAM: Class = (
        "Windows.Storage.Streams.FileOutputStream",
        Guid::parse("905a0fe6-bc53-11df-8c49-001e4fc686da"),
    );
    pub const FILE_RANDOM_ACCESS_STREAM: Class = (
        "Windows.Storage.Streams.FileRandomAccessStream",
        Guid::parse("905a0fe1-bc53-11df-8c49-001e4fc686da"),
    );
    pub const GATT_CHARACTERISTIC: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattCharacteristic",
        Guid::parse("59cb50c1-5934-4f68-a198-eb864fa44e6b"),
    );
    pub const GATT_CHARACTERISTICS_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattCharacteristicsResult",
        Guid::parse("1194945c-b257-4f3e-9db7-f68bc9a9aef2"),
    );
    pub const GATT_CLIENT_NOTIFICATION_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattClientNotificationResult",
        Guid::parse("506d5599-0112-419a-8e3b-ae21afabd2c2"),
    );
    pub const GATT_DESCRIPTOR: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattDescriptor",
        Guid::parse("92055f2b-8084-4344-b4c2-284de19a8506"),
    );
    pub const GATT_DESCRIPTORS_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattDescriptorsResult",
        Guid::parse("9bc091f3-95e7-4489-8d25-ff81955a57b9"),
    );
    pub const GATT_DEVICE_SERVICE: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattDeviceService",
        Guid::parse("ac7b7c05-b33c-47cf-990f-6b8f5577df71"),
    );
    pub const GATT_DEVICE_SERVICES_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattDeviceServicesResult",
        Guid::parse("171dd3ee-016d-419d-838a-576cf475a3d8"),
    );
    pub const GATT_LOCAL_CHARACTERISTIC: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalCharacteristic",
        Guid::parse("aede376d-5412-4d74-92a8-8deb8526829c"),
    );
    pub const GATT_LOCAL_CHARACTERISTIC_PARAMETERS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalCharacteristicParameters",
        Guid::parse("faf73db4-4cff-44c7-8445-040e6ead0063"),
    );
    pub const GATT_LOCAL_CHARACTERISTIC_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalCharacteristicResult",
        Guid::parse("7975de9b-0170-4397-9666-92f863f12ee6"),
    );
    pub const GATT_LOCAL_DESCRIPTOR: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalDescriptor",
        Guid::parse("f48ebe06-789d-4a4b-8652-bd017b5d2fc6"),
    );
    pub const GATT_LOCAL_DESCRIPTOR_PARAMETERS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalDescriptorParameters",
        Guid::parse("5fdede6a-f3c1-4b66-8c4b-e3d2293b40e9"),
    );
    pub const GATT_LOCAL_DESCRIPTOR_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalDescriptorResult",
        Guid::parse("375791be-321f-4366-bfc1-3bc6b82c79f8"),
    );
    pub const GATT_LOCAL_SERVICE: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattLocalService",
        Guid::parse("f513e258-f7f7-4902-b803-57fcc7d6fe83"),
    );
    pub const GATT_PRESENTATION_FORMAT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattPresentationFormat",
        Guid::parse("196d0021-faad-45dc-ae5b-2ac3184e84db"),
    );
    pub const GATT_READ_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_RESULT: Class = ("Windows.Devices.Bluetooth.GenericAttributeProfile.GattReadClientCharacteristicConfigurationDescriptorResult", Guid::parse("63a66f09-1aea-4c4c-a50f-97bae474b348"));
    pub const GATT_READ_REQUEST: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattReadRequest",
        Guid::parse("f1dd6535-6acd-42a6-a4bb-d789dae0043e"),
    );
    pub const GATT_READ_REQUESTED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattReadRequestedEventArgs",
        Guid::parse("93497243-f39c-484b-8ab6-996ba486cfa3"),
    );
    pub const GATT_READ_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattReadResult",
        Guid::parse("63a66f08-1aea-4c4c-a50f-97bae474b348"),
    );
    pub const GATT_RELIABLE_WRITE_TRANSACTION: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattReliableWriteTransaction",
        Guid::parse("63a66f07-1aea-4c4c-a50f-97bae474b348"),
    );
    pub const GATT_REQUEST_STATE_CHANGED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattRequestStateChangedEventArgs",
        Guid::parse("e834d92c-27be-44b3-9d0d-4fc6e808dd3f"),
    );
    pub const GATT_SERVICE_PROVIDER: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattServiceProvider",
        Guid::parse("7822b3cd-2889-4f86-a051-3f0aed1c2760"),
    );
    pub const GATT_SERVICE_PROVIDER_ADVERTISEMENT_STATUS_CHANGED_EVENT_ARGS: Class = ("Windows.Devices.Bluetooth.GenericAttributeProfile.GattServiceProviderAdvertisementStatusChangedEventArgs", Guid::parse("59a5aa65-fa21-4ffc-b155-04d928012686"));
    pub const GATT_SERVICE_PROVIDER_ADVERTISING_PARAMETERS: Class = ("Windows.Devices.Bluetooth.GenericAttributeProfile.GattServiceProviderAdvertisingParameters", Guid::parse("e2ce31ab-6315-4c22-9bd7-781dbc3d8d82"));
    pub const GATT_SERVICE_PROVIDER_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattServiceProviderResult",
        Guid::parse("764696d8-c53e-428c-8a48-67afe02c3ae6"),
    );
    pub const GATT_SESSION: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattSession",
        Guid::parse("d23b5143-e04e-4c24-999c-9c256f9856b1"),
    );
    pub const GATT_SESSION_STATUS_CHANGED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattSessionStatusChangedEventArgs",
        Guid::parse("7605b72e-837f-404c-ab34-3163f39ddf32"),
    );
    pub const GATT_SUBSCRIBED_CLIENT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattSubscribedClient",
        Guid::parse("736e9001-15a4-4ec2-9248-e3f20d463be9"),
    );
    pub const GATT_VALUE_CHANGED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattValueChangedEventArgs",
        Guid::parse("d21bdb54-06e3-4ed8-a263-acfac8ba7313"),
    );
    pub const GATT_WRITE_REQUEST: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattWriteRequest",
        Guid::parse("aeb6a9ed-de2f-4fc2-a9a8-94ea7844f13d"),
    );
    pub const GATT_WRITE_REQUESTED_EVENT_ARGS: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattWriteRequestedEventArgs",
        Guid::parse("2dec8bbe-a73a-471a-94d5-037deadd0806"),
    );
    pub const GATT_WRITE_RESULT: Class = (
        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattWriteResult",
        Guid::parse("4991ddb1-cb2b-44f7-99fc-d29a2871dc9b"),
    );
    pub const IN_MEMORY_RANDOM_ACCESS_STREAM: Class = (
        "Windows.Storage.Streams.InMemoryRandomAccessStream",
        Guid::parse("905a0fe1-bc53-11df-8c49-001e4fc686da"),
    );
    pub const INPUT_STREAM_OVER_STREAM: Class = (
        "Windows.Storage.Streams.InputStreamOverStream",
        Guid::parse("905a0fe2-bc53-11df-8c49-001e4fc686da"),
    );
    pub const MEMORY_BUFFER: Class = (
        "Windows.Foundation.MemoryBuffer",
        Guid::parse("fbc4dd2a-245b-11e4-af98-689423260cf8"),
    );
    pub const OUTPUT_STREAM_OVER_STREAM: Class = (
        "Windows.Storage.Streams.OutputStreamOverStream",
        Guid::parse("905a0fe6-bc53-11df-8c49-001e4fc686da"),
    );
    pub const RANDOM_ACCESS_STREAM_OVER_STREAM: Class = (
        "Windows.Storage.Streams.RandomAccessStreamOverStream",
        Guid::parse("905a0fe1-bc53-11df-8c49-001e4fc686da"),
    );
    pub const RANDOM_ACCESS_STREAM_REFERENCE: Class = (
        "Windows.Storage.Streams.RandomAccessStreamReference",
        Guid::parse("33ee3134-1dd6-4e3a-8067-d1c162e8642b"),
    );
    pub const URI: Class = (
        "Windows.Foundation.Uri",
        Guid::parse("9e365e57-48b2-4160-956f-c7385120bbfc"),
    );
    pub const WWW_FORM_URL_DECODER: Class = (
        "Windows.Foundation.WwwFormUrlDecoder",
        Guid::parse("d45a0451-f225-4542-9296-0e1df5d254df"),
    );
    pub const WWW_FORM_URL_DECODER_ENTRY: Class = (
        "Windows.Foundation.WwwFormUrlDecoderEntry",
        Guid::parse("125e7431-f678-4e8e-b670-20a9b06c512d"),
    );
}

/// Vtable slot indices, in the order the metadata declares the methods.
///
/// A method called one slot out jumps into a different function with the
/// wrong arguments, and nothing here would catch it — so these are
/// generated from the same metadata as the IIDs rather than counted by
/// hand.
pub mod slots {
    /// `IAsyncInfo`
    pub mod iasync_info {
        pub const ID: usize = 6;
        pub const STATUS: usize = 7;
        pub const ERROR_CODE: usize = 8;
        pub const CANCEL: usize = 9;
        pub const CLOSE: usize = 10;
    }
    /// `IAsyncOperation`
    pub mod iasync_operation {
        pub const SET_COMPLETED: usize = 6;
        pub const COMPLETED: usize = 7;
        pub const GET_RESULTS: usize = 8;
    }
    /// `IBluetoothAdapter`
    pub mod ibluetooth_adapter {
        pub const DEVICE_ID: usize = 6;
        pub const BLUETOOTH_ADDRESS: usize = 7;
        pub const IS_CLASSIC_SUPPORTED: usize = 8;
        pub const IS_LOW_ENERGY_SUPPORTED: usize = 9;
        pub const IS_PERIPHERAL_ROLE_SUPPORTED: usize = 10;
        pub const IS_CENTRAL_ROLE_SUPPORTED: usize = 11;
        pub const IS_ADVERTISEMENT_OFFLOAD_SUPPORTED: usize = 12;
        pub const GET_RADIO_ASYNC: usize = 13;
    }
    /// `IBluetoothAdapterStatics`
    pub mod ibluetooth_adapter_statics {
        pub const GET_DEVICE_SELECTOR: usize = 6;
        pub const FROM_ID_ASYNC: usize = 7;
        pub const GET_DEFAULT_ASYNC: usize = 8;
    }
    /// `IBluetoothLEAdvertisement`
    pub mod ibluetooth_leadvertisement {
        pub const FLAGS: usize = 6;
        pub const SET_FLAGS: usize = 7;
        pub const LOCAL_NAME: usize = 8;
        pub const SET_LOCAL_NAME: usize = 9;
        pub const SERVICE_UUIDS: usize = 10;
        pub const MANUFACTURER_DATA: usize = 11;
        pub const DATA_SECTIONS: usize = 12;
        pub const GET_MANUFACTURER_DATA_BY_COMPANY_ID: usize = 13;
        pub const GET_SECTIONS_BY_TYPE: usize = 14;
    }
    /// `IBluetoothLEAdvertisementPublisher`
    pub mod ibluetooth_leadvertisement_publisher {
        pub const STATUS: usize = 6;
        pub const ADVERTISEMENT: usize = 7;
        pub const START: usize = 8;
        pub const STOP: usize = 9;
        pub const STATUS_CHANGED: usize = 10;
        pub const REMOVE_STATUS_CHANGED: usize = 11;
    }
    /// `IBluetoothLEAdvertisementPublisherStatusChangedEventArgs`
    pub mod ibluetooth_leadvertisement_publisher_status_changed_event_args {
        pub const STATUS: usize = 6;
        pub const ERROR: usize = 7;
    }
    /// `IBluetoothLEAdvertisementReceivedEventArgs`
    pub mod ibluetooth_leadvertisement_received_event_args {
        pub const RAW_SIGNAL_STRENGTH_IN_D_BM: usize = 6;
        pub const BLUETOOTH_ADDRESS: usize = 7;
        pub const ADVERTISEMENT_TYPE: usize = 8;
        pub const TIMESTAMP: usize = 9;
        pub const ADVERTISEMENT: usize = 10;
    }
    /// `IBluetoothLEAdvertisementWatcher`
    pub mod ibluetooth_leadvertisement_watcher {
        pub const MIN_SAMPLING_INTERVAL: usize = 6;
        pub const MAX_SAMPLING_INTERVAL: usize = 7;
        pub const MIN_OUT_OF_RANGE_TIMEOUT: usize = 8;
        pub const MAX_OUT_OF_RANGE_TIMEOUT: usize = 9;
        pub const STATUS: usize = 10;
        pub const SCANNING_MODE: usize = 11;
        pub const SET_SCANNING_MODE: usize = 12;
        pub const SIGNAL_STRENGTH_FILTER: usize = 13;
        pub const SET_SIGNAL_STRENGTH_FILTER: usize = 14;
        pub const ADVERTISEMENT_FILTER: usize = 15;
        pub const SET_ADVERTISEMENT_FILTER: usize = 16;
        pub const START: usize = 17;
        pub const STOP: usize = 18;
        pub const RECEIVED: usize = 19;
        pub const REMOVE_RECEIVED: usize = 20;
        pub const STOPPED: usize = 21;
        pub const REMOVE_STOPPED: usize = 22;
    }
    /// `IBluetoothLEConnectionParameters`
    pub mod ibluetooth_leconnection_parameters {
        pub const LINK_TIMEOUT: usize = 6;
        pub const CONNECTION_LATENCY: usize = 7;
        pub const CONNECTION_INTERVAL: usize = 8;
    }
    /// `IBluetoothLEConnectionPhy`
    pub mod ibluetooth_leconnection_phy {
        pub const TRANSMIT_INFO: usize = 6;
        pub const RECEIVE_INFO: usize = 7;
    }
    /// `IBluetoothLEConnectionPhyInfo`
    pub mod ibluetooth_leconnection_phy_info {
        pub const IS_UNCODED1_M_PHY: usize = 6;
        pub const IS_UNCODED2_M_PHY: usize = 7;
        pub const IS_CODED_PHY: usize = 8;
    }
    /// `IBluetoothLEDevice`
    pub mod ibluetooth_ledevice {
        pub const DEVICE_ID: usize = 6;
        pub const NAME: usize = 7;
        pub const GATT_SERVICES: usize = 8;
        pub const CONNECTION_STATUS: usize = 9;
        pub const BLUETOOTH_ADDRESS: usize = 10;
        pub const GET_GATT_SERVICE: usize = 11;
        pub const NAME_CHANGED: usize = 12;
        pub const REMOVE_NAME_CHANGED: usize = 13;
        pub const GATT_SERVICES_CHANGED: usize = 14;
        pub const REMOVE_GATT_SERVICES_CHANGED: usize = 15;
        pub const CONNECTION_STATUS_CHANGED: usize = 16;
        pub const REMOVE_CONNECTION_STATUS_CHANGED: usize = 17;
    }
    /// `IBluetoothLEDevice2`
    pub mod ibluetooth_ledevice2 {
        pub const DEVICE_INFORMATION: usize = 6;
        pub const APPEARANCE: usize = 7;
        pub const BLUETOOTH_ADDRESS_TYPE: usize = 8;
    }
    /// `IBluetoothLEDevice3`
    pub mod ibluetooth_ledevice3 {
        pub const DEVICE_ACCESS_INFORMATION: usize = 6;
        pub const REQUEST_ACCESS_ASYNC: usize = 7;
        pub const GET_GATT_SERVICES_ASYNC: usize = 8;
        pub const GET_GATT_SERVICES_WITH_CACHE_MODE_ASYNC: usize = 9;
        pub const GET_GATT_SERVICES_FOR_UUID_ASYNC: usize = 10;
        pub const GET_GATT_SERVICES_FOR_UUID_WITH_CACHE_MODE_ASYNC: usize = 11;
    }
    /// `IBluetoothLEDevice4`
    pub mod ibluetooth_ledevice4 {
        pub const BLUETOOTH_DEVICE_ID: usize = 6;
    }
    /// `IBluetoothLEDevice5`
    pub mod ibluetooth_ledevice5 {
        pub const WAS_SECURE_CONNECTION_USED_FOR_PAIRING: usize = 6;
    }
    /// `IBluetoothLEDevice6`
    pub mod ibluetooth_ledevice6 {
        pub const GET_CONNECTION_PARAMETERS: usize = 6;
        pub const GET_CONNECTION_PHY: usize = 7;
        pub const REQUEST_PREFERRED_CONNECTION_PARAMETERS: usize = 8;
        pub const CONNECTION_PARAMETERS_CHANGED: usize = 9;
        pub const REMOVE_CONNECTION_PARAMETERS_CHANGED: usize = 10;
        pub const CONNECTION_PHY_CHANGED: usize = 11;
        pub const REMOVE_CONNECTION_PHY_CHANGED: usize = 12;
    }
    /// `IBluetoothLEDeviceStatics`
    pub mod ibluetooth_ledevice_statics {
        pub const FROM_ID_ASYNC: usize = 6;
        pub const FROM_BLUETOOTH_ADDRESS_ASYNC: usize = 7;
        pub const GET_DEVICE_SELECTOR: usize = 8;
    }
    /// `IBluetoothLEDeviceStatics2`
    pub mod ibluetooth_ledevice_statics2 {
        pub const GET_DEVICE_SELECTOR_FROM_PAIRING_STATE: usize = 6;
        pub const GET_DEVICE_SELECTOR_FROM_CONNECTION_STATUS: usize = 7;
        pub const GET_DEVICE_SELECTOR_FROM_DEVICE_NAME: usize = 8;
        pub const GET_DEVICE_SELECTOR_FROM_BLUETOOTH_ADDRESS: usize = 9;
        pub const GET_DEVICE_SELECTOR_FROM_BLUETOOTH_ADDRESS_WITH_BLUETOOTH_ADDRESS_TYPE: usize =
            10;
        pub const GET_DEVICE_SELECTOR_FROM_APPEARANCE: usize = 11;
        pub const FROM_BLUETOOTH_ADDRESS_WITH_BLUETOOTH_ADDRESS_TYPE_ASYNC: usize = 12;
    }
    /// `IBluetoothLEManufacturerData`
    pub mod ibluetooth_lemanufacturer_data {
        pub const COMPANY_ID: usize = 6;
        pub const SET_COMPANY_ID: usize = 7;
        pub const DATA: usize = 8;
        pub const SET_DATA: usize = 9;
    }
    /// `IBluetoothLEPreferredConnectionParameters`
    pub mod ibluetooth_lepreferred_connection_parameters {
        pub const LINK_TIMEOUT: usize = 6;
        pub const CONNECTION_LATENCY: usize = 7;
        pub const MIN_CONNECTION_INTERVAL: usize = 8;
        pub const MAX_CONNECTION_INTERVAL: usize = 9;
    }
    /// `IBluetoothLEPreferredConnectionParametersRequest`
    pub mod ibluetooth_lepreferred_connection_parameters_request {
        pub const STATUS: usize = 6;
    }
    /// `IBluetoothLEPreferredConnectionParametersStatics`
    pub mod ibluetooth_lepreferred_connection_parameters_statics {
        pub const BALANCED: usize = 6;
        pub const THROUGHPUT_OPTIMIZED: usize = 7;
        pub const POWER_OPTIMIZED: usize = 8;
    }
    /// `IBuffer`
    pub mod ibuffer {
        pub const CAPACITY: usize = 6;
        pub const LENGTH: usize = 7;
        pub const SET_LENGTH: usize = 8;
    }
    /// `IClosable`
    pub mod iclosable {
        pub const CLOSE: usize = 6;
    }
    /// `IDataReader`
    pub mod idata_reader {
        pub const UNCONSUMED_BUFFER_LENGTH: usize = 6;
        pub const UNICODE_ENCODING: usize = 7;
        pub const SET_UNICODE_ENCODING: usize = 8;
        pub const BYTE_ORDER: usize = 9;
        pub const SET_BYTE_ORDER: usize = 10;
        pub const INPUT_STREAM_OPTIONS: usize = 11;
        pub const SET_INPUT_STREAM_OPTIONS: usize = 12;
        pub const READ_BYTE: usize = 13;
        pub const READ_BYTES: usize = 14;
        pub const READ_BUFFER: usize = 15;
        pub const READ_BOOLEAN: usize = 16;
        pub const READ_GUID: usize = 17;
        pub const READ_INT16: usize = 18;
        pub const READ_INT32: usize = 19;
        pub const READ_INT64: usize = 20;
        pub const READ_U_INT16: usize = 21;
        pub const READ_U_INT32: usize = 22;
        pub const READ_U_INT64: usize = 23;
        pub const READ_SINGLE: usize = 24;
        pub const READ_DOUBLE: usize = 25;
        pub const READ_STRING: usize = 26;
        pub const READ_DATE_TIME: usize = 27;
        pub const READ_TIME_SPAN: usize = 28;
        pub const LOAD_ASYNC: usize = 29;
        pub const DETACH_BUFFER: usize = 30;
        pub const DETACH_STREAM: usize = 31;
    }
    /// `IDataReaderStatics`
    pub mod idata_reader_statics {
        pub const FROM_BUFFER: usize = 6;
    }
    /// `IDataWriter`
    pub mod idata_writer {
        pub const UNSTORED_BUFFER_LENGTH: usize = 6;
        pub const UNICODE_ENCODING: usize = 7;
        pub const SET_UNICODE_ENCODING: usize = 8;
        pub const BYTE_ORDER: usize = 9;
        pub const SET_BYTE_ORDER: usize = 10;
        pub const WRITE_BYTE: usize = 11;
        pub const WRITE_BYTES: usize = 12;
        pub const WRITE_BUFFER: usize = 13;
        pub const WRITE_BUFFER_RANGE: usize = 14;
        pub const WRITE_BOOLEAN: usize = 15;
        pub const WRITE_GUID: usize = 16;
        pub const WRITE_INT16: usize = 17;
        pub const WRITE_INT32: usize = 18;
        pub const WRITE_INT64: usize = 19;
        pub const WRITE_U_INT16: usize = 20;
        pub const WRITE_U_INT32: usize = 21;
        pub const WRITE_U_INT64: usize = 22;
        pub const WRITE_SINGLE: usize = 23;
        pub const WRITE_DOUBLE: usize = 24;
        pub const WRITE_DATE_TIME: usize = 25;
        pub const WRITE_TIME_SPAN: usize = 26;
        pub const WRITE_STRING: usize = 27;
        pub const MEASURE_STRING: usize = 28;
        pub const STORE_ASYNC: usize = 29;
        pub const FLUSH_ASYNC: usize = 30;
        pub const DETACH_BUFFER: usize = 31;
        pub const DETACH_STREAM: usize = 32;
    }
    /// `IDeferral`
    pub mod ideferral {
        pub const COMPLETE: usize = 6;
    }
    /// `IDeviceInformation`
    pub mod idevice_information {
        pub const ID: usize = 6;
        pub const NAME: usize = 7;
        pub const IS_ENABLED: usize = 8;
        pub const IS_DEFAULT: usize = 9;
        pub const ENCLOSURE_LOCATION: usize = 10;
        pub const PROPERTIES: usize = 11;
        pub const UPDATE: usize = 12;
        pub const GET_THUMBNAIL_ASYNC: usize = 13;
        pub const GET_GLYPH_THUMBNAIL_ASYNC: usize = 14;
    }
    /// `IDeviceInformation2`
    pub mod idevice_information2 {
        pub const KIND: usize = 6;
        pub const PAIRING: usize = 7;
    }
    /// `IDeviceInformationPairing`
    pub mod idevice_information_pairing {
        pub const IS_PAIRED: usize = 6;
        pub const CAN_PAIR: usize = 7;
        pub const PAIR_ASYNC: usize = 8;
        pub const PAIR_WITH_PROTECTION_LEVEL_ASYNC: usize = 9;
    }
    /// `IDeviceInformationPairing2`
    pub mod idevice_information_pairing2 {
        pub const PROTECTION_LEVEL: usize = 6;
        pub const CUSTOM: usize = 7;
        pub const PAIR_WITH_PROTECTION_LEVEL_AND_SETTINGS_ASYNC: usize = 8;
        pub const UNPAIR_ASYNC: usize = 9;
    }
    /// `IDeviceInformationPairingStatics`
    pub mod idevice_information_pairing_statics {
        pub const TRY_REGISTER_FOR_ALL_INBOUND_PAIRING_REQUESTS: usize = 6;
    }
    /// `IDeviceInformationStatics`
    pub mod idevice_information_statics {
        pub const CREATE_FROM_ID_ASYNC: usize = 6;
        pub const CREATE_FROM_ID_ASYNC_ADDITIONAL_PROPERTIES: usize = 7;
        pub const FIND_ALL_ASYNC: usize = 8;
        pub const FIND_ALL_ASYNC_DEVICE_CLASS: usize = 9;
        pub const FIND_ALL_ASYNC_AQS_FILTER: usize = 10;
        pub const FIND_ALL_ASYNC_AQS_FILTER_AND_ADDITIONAL_PROPERTIES: usize = 11;
        pub const CREATE_WATCHER: usize = 12;
        pub const CREATE_WATCHER_DEVICE_CLASS: usize = 13;
        pub const CREATE_WATCHER_AQS_FILTER: usize = 14;
        pub const CREATE_WATCHER_AQS_FILTER_AND_ADDITIONAL_PROPERTIES: usize = 15;
    }
    /// `IDevicePairingResult`
    pub mod idevice_pairing_result {
        pub const STATUS: usize = 6;
        pub const PROTECTION_LEVEL_USED: usize = 7;
    }
    /// `IGattCharacteristic`
    pub mod igatt_characteristic {
        pub const GET_DESCRIPTORS: usize = 6;
        pub const CHARACTERISTIC_PROPERTIES: usize = 7;
        pub const PROTECTION_LEVEL: usize = 8;
        pub const SET_PROTECTION_LEVEL: usize = 9;
        pub const USER_DESCRIPTION: usize = 10;
        pub const UUID: usize = 11;
        pub const ATTRIBUTE_HANDLE: usize = 12;
        pub const PRESENTATION_FORMATS: usize = 13;
        pub const READ_VALUE_ASYNC: usize = 14;
        pub const READ_VALUE_WITH_CACHE_MODE_ASYNC: usize = 15;
        pub const WRITE_VALUE_ASYNC: usize = 16;
        pub const WRITE_VALUE_WITH_OPTION_ASYNC: usize = 17;
        pub const READ_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_ASYNC: usize = 18;
        pub const WRITE_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_ASYNC: usize = 19;
        pub const VALUE_CHANGED: usize = 20;
        pub const REMOVE_VALUE_CHANGED: usize = 21;
    }
    /// `IGattCharacteristic3`
    pub mod igatt_characteristic3 {
        pub const GET_DESCRIPTORS_ASYNC: usize = 6;
        pub const GET_DESCRIPTORS_WITH_CACHE_MODE_ASYNC: usize = 7;
        pub const GET_DESCRIPTORS_FOR_UUID_ASYNC: usize = 8;
        pub const GET_DESCRIPTORS_FOR_UUID_WITH_CACHE_MODE_ASYNC: usize = 9;
        pub const WRITE_VALUE_WITH_RESULT_ASYNC: usize = 10;
        pub const WRITE_VALUE_WITH_RESULT_AND_OPTION_ASYNC: usize = 11;
        pub const WRITE_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_WITH_RESULT_ASYNC: usize =
            12;
    }
    /// `IGattCharacteristicsResult`
    pub mod igatt_characteristics_result {
        pub const STATUS: usize = 6;
        pub const PROTOCOL_ERROR: usize = 7;
        pub const CHARACTERISTICS: usize = 8;
    }
    /// `IGattClientNotificationResult`
    pub mod igatt_client_notification_result {
        pub const SUBSCRIBED_CLIENT: usize = 6;
        pub const STATUS: usize = 7;
        pub const PROTOCOL_ERROR: usize = 8;
    }
    /// `IGattDescriptor`
    pub mod igatt_descriptor {
        pub const PROTECTION_LEVEL: usize = 6;
        pub const SET_PROTECTION_LEVEL: usize = 7;
        pub const UUID: usize = 8;
        pub const ATTRIBUTE_HANDLE: usize = 9;
        pub const READ_VALUE_ASYNC: usize = 10;
        pub const READ_VALUE_WITH_CACHE_MODE_ASYNC: usize = 11;
        pub const WRITE_VALUE_ASYNC: usize = 12;
    }
    /// `IGattDescriptorsResult`
    pub mod igatt_descriptors_result {
        pub const STATUS: usize = 6;
        pub const PROTOCOL_ERROR: usize = 7;
        pub const DESCRIPTORS: usize = 8;
    }
    /// `IGattDeviceService`
    pub mod igatt_device_service {
        pub const GET_CHARACTERISTICS: usize = 6;
        pub const GET_INCLUDED_SERVICES: usize = 7;
        pub const DEVICE_ID: usize = 8;
        pub const UUID: usize = 9;
        pub const ATTRIBUTE_HANDLE: usize = 10;
    }
    /// `IGattDeviceService3`
    pub mod igatt_device_service3 {
        pub const DEVICE_ACCESS_INFORMATION: usize = 6;
        pub const SESSION: usize = 7;
        pub const SHARING_MODE: usize = 8;
        pub const REQUEST_ACCESS_ASYNC: usize = 9;
        pub const OPEN_ASYNC: usize = 10;
        pub const GET_CHARACTERISTICS_ASYNC: usize = 11;
        pub const GET_CHARACTERISTICS_WITH_CACHE_MODE_ASYNC: usize = 12;
        pub const GET_CHARACTERISTICS_FOR_UUID_ASYNC: usize = 13;
        pub const GET_CHARACTERISTICS_FOR_UUID_WITH_CACHE_MODE_ASYNC: usize = 14;
        pub const GET_INCLUDED_SERVICES_ASYNC: usize = 15;
        pub const GET_INCLUDED_SERVICES_WITH_CACHE_MODE_ASYNC: usize = 16;
        pub const GET_INCLUDED_SERVICES_FOR_UUID_ASYNC: usize = 17;
        pub const GET_INCLUDED_SERVICES_FOR_UUID_WITH_CACHE_MODE_ASYNC: usize = 18;
    }
    /// `IGattDeviceServicesResult`
    pub mod igatt_device_services_result {
        pub const STATUS: usize = 6;
        pub const PROTOCOL_ERROR: usize = 7;
        pub const SERVICES: usize = 8;
    }
    /// `IGattLocalCharacteristic`
    pub mod igatt_local_characteristic {
        pub const UUID: usize = 6;
        pub const STATIC_VALUE: usize = 7;
        pub const CHARACTERISTIC_PROPERTIES: usize = 8;
        pub const READ_PROTECTION_LEVEL: usize = 9;
        pub const WRITE_PROTECTION_LEVEL: usize = 10;
        pub const CREATE_DESCRIPTOR_ASYNC: usize = 11;
        pub const DESCRIPTORS: usize = 12;
        pub const USER_DESCRIPTION: usize = 13;
        pub const PRESENTATION_FORMATS: usize = 14;
        pub const SUBSCRIBED_CLIENTS: usize = 15;
        pub const SUBSCRIBED_CLIENTS_CHANGED: usize = 16;
        pub const REMOVE_SUBSCRIBED_CLIENTS_CHANGED: usize = 17;
        pub const READ_REQUESTED: usize = 18;
        pub const REMOVE_READ_REQUESTED: usize = 19;
        pub const WRITE_REQUESTED: usize = 20;
        pub const REMOVE_WRITE_REQUESTED: usize = 21;
        pub const NOTIFY_VALUE_ASYNC: usize = 22;
        pub const NOTIFY_VALUE_FOR_SUBSCRIBED_CLIENT_ASYNC: usize = 23;
    }
    /// `IGattLocalCharacteristicParameters`
    pub mod igatt_local_characteristic_parameters {
        pub const SET_STATIC_VALUE: usize = 6;
        pub const STATIC_VALUE: usize = 7;
        pub const SET_CHARACTERISTIC_PROPERTIES: usize = 8;
        pub const CHARACTERISTIC_PROPERTIES: usize = 9;
        pub const SET_READ_PROTECTION_LEVEL: usize = 10;
        pub const READ_PROTECTION_LEVEL: usize = 11;
        pub const SET_WRITE_PROTECTION_LEVEL: usize = 12;
        pub const WRITE_PROTECTION_LEVEL: usize = 13;
        pub const SET_USER_DESCRIPTION: usize = 14;
        pub const USER_DESCRIPTION: usize = 15;
        pub const PRESENTATION_FORMATS: usize = 16;
    }
    /// `IGattLocalCharacteristicResult`
    pub mod igatt_local_characteristic_result {
        pub const CHARACTERISTIC: usize = 6;
        pub const ERROR: usize = 7;
    }
    /// `IGattLocalDescriptorParameters`
    pub mod igatt_local_descriptor_parameters {
        pub const SET_STATIC_VALUE: usize = 6;
        pub const STATIC_VALUE: usize = 7;
        pub const SET_READ_PROTECTION_LEVEL: usize = 8;
        pub const READ_PROTECTION_LEVEL: usize = 9;
        pub const SET_WRITE_PROTECTION_LEVEL: usize = 10;
        pub const WRITE_PROTECTION_LEVEL: usize = 11;
    }
    /// `IGattLocalService`
    pub mod igatt_local_service {
        pub const UUID: usize = 6;
        pub const CREATE_CHARACTERISTIC_ASYNC: usize = 7;
        pub const CHARACTERISTICS: usize = 8;
    }
    /// `IGattReadRequest`
    pub mod igatt_read_request {
        pub const OFFSET: usize = 6;
        pub const LENGTH: usize = 7;
        pub const STATE: usize = 8;
        pub const STATE_CHANGED: usize = 9;
        pub const REMOVE_STATE_CHANGED: usize = 10;
        pub const RESPOND_WITH_VALUE: usize = 11;
        pub const RESPOND_WITH_PROTOCOL_ERROR: usize = 12;
    }
    /// `IGattReadRequestedEventArgs`
    pub mod igatt_read_requested_event_args {
        pub const SESSION: usize = 6;
        pub const GET_DEFERRAL: usize = 7;
        pub const GET_REQUEST_ASYNC: usize = 8;
    }
    /// `IGattReadResult`
    pub mod igatt_read_result {
        pub const STATUS: usize = 6;
        pub const VALUE: usize = 7;
    }
    /// `IGattServiceProvider`
    pub mod igatt_service_provider {
        pub const SERVICE: usize = 6;
        pub const ADVERTISEMENT_STATUS: usize = 7;
        pub const ADVERTISEMENT_STATUS_CHANGED: usize = 8;
        pub const REMOVE_ADVERTISEMENT_STATUS_CHANGED: usize = 9;
        pub const START_ADVERTISING: usize = 10;
        pub const START_ADVERTISING_WITH_PARAMETERS: usize = 11;
        pub const STOP_ADVERTISING: usize = 12;
    }
    /// `IGattServiceProviderAdvertisingParameters`
    pub mod igatt_service_provider_advertising_parameters {
        pub const SET_IS_CONNECTABLE: usize = 6;
        pub const IS_CONNECTABLE: usize = 7;
        pub const SET_IS_DISCOVERABLE: usize = 8;
        pub const IS_DISCOVERABLE: usize = 9;
    }
    /// `IGattServiceProviderResult`
    pub mod igatt_service_provider_result {
        pub const ERROR: usize = 6;
        pub const SERVICE_PROVIDER: usize = 7;
    }
    /// `IGattServiceProviderStatics`
    pub mod igatt_service_provider_statics {
        pub const CREATE_ASYNC: usize = 6;
    }
    /// `IGattSession`
    pub mod igatt_session {
        pub const DEVICE_ID: usize = 6;
        pub const CAN_MAINTAIN_CONNECTION: usize = 7;
        pub const SET_MAINTAIN_CONNECTION: usize = 8;
        pub const MAINTAIN_CONNECTION: usize = 9;
        pub const MAX_PDU_SIZE: usize = 10;
        pub const SESSION_STATUS: usize = 11;
        pub const MAX_PDU_SIZE_CHANGED: usize = 12;
        pub const REMOVE_MAX_PDU_SIZE_CHANGED: usize = 13;
        pub const SESSION_STATUS_CHANGED: usize = 14;
        pub const REMOVE_SESSION_STATUS_CHANGED: usize = 15;
    }
    /// `IGattSessionStatics`
    pub mod igatt_session_statics {
        pub const FROM_DEVICE_ID_ASYNC: usize = 6;
    }
    /// `IGattSubscribedClient`
    pub mod igatt_subscribed_client {
        pub const SESSION: usize = 6;
        pub const MAX_NOTIFICATION_SIZE: usize = 7;
        pub const MAX_NOTIFICATION_SIZE_CHANGED: usize = 8;
        pub const REMOVE_MAX_NOTIFICATION_SIZE_CHANGED: usize = 9;
    }
    /// `IGattValueChangedEventArgs`
    pub mod igatt_value_changed_event_args {
        pub const CHARACTERISTIC_VALUE: usize = 6;
        pub const TIMESTAMP: usize = 7;
    }
    /// `IGattWriteRequest`
    pub mod igatt_write_request {
        pub const VALUE: usize = 6;
        pub const OFFSET: usize = 7;
        pub const OPTION: usize = 8;
        pub const STATE: usize = 9;
        pub const STATE_CHANGED: usize = 10;
        pub const REMOVE_STATE_CHANGED: usize = 11;
        pub const RESPOND: usize = 12;
        pub const RESPOND_WITH_PROTOCOL_ERROR: usize = 13;
    }
    /// `IGattWriteRequestedEventArgs`
    pub mod igatt_write_requested_event_args {
        pub const SESSION: usize = 6;
        pub const GET_DEFERRAL: usize = 7;
        pub const GET_REQUEST_ASYNC: usize = 8;
    }
    /// `IGattWriteResult`
    pub mod igatt_write_result {
        pub const STATUS: usize = 6;
        pub const PROTOCOL_ERROR: usize = 7;
    }
    /// `IIterable`
    pub mod iiterable {
        pub const FIRST: usize = 6;
    }
    /// `IIterator`
    pub mod iiterator {
        pub const CURRENT: usize = 6;
        pub const HAS_CURRENT: usize = 7;
        pub const MOVE_NEXT: usize = 8;
        pub const GET_MANY: usize = 9;
    }
    /// `IVector`
    pub mod ivector {
        pub const GET_AT: usize = 6;
        pub const SIZE: usize = 7;
        pub const GET_VIEW: usize = 8;
        pub const INDEX_OF: usize = 9;
        pub const SET_AT: usize = 10;
        pub const INSERT_AT: usize = 11;
        pub const REMOVE_AT: usize = 12;
        pub const APPEND: usize = 13;
        pub const REMOVE_AT_END: usize = 14;
        pub const CLEAR: usize = 15;
        pub const GET_MANY: usize = 16;
        pub const REPLACE_ALL: usize = 17;
    }
    /// `IVectorView`
    pub mod ivector_view {
        pub const GET_AT: usize = 6;
        pub const SIZE: usize = 7;
        pub const INDEX_OF: usize = 8;
        pub const GET_MANY: usize = 9;
    }
}

/// Parameterised types, by the GUID of the generic itself.
pub mod generics {
    use super::Guid;
    pub const ASYNC_ACTION_PROGRESS_HANDLER: Guid =
        Guid::parse("6d844858-0cff-4590-ae89-95a5a5c8b4b8");
    pub const ASYNC_ACTION_WITH_PROGRESS_COMPLETED_HANDLER: Guid =
        Guid::parse("9c029f91-cc84-44fd-ac26-0a6c4e555281");
    pub const ASYNC_OPERATION_COMPLETED_HANDLER: Guid =
        Guid::parse("fcdcf02c-e5d8-4478-915a-4d90b74b83a5");
    pub const ASYNC_OPERATION_PROGRESS_HANDLER: Guid =
        Guid::parse("55690902-0aab-421a-8778-f8ce5026d758");
    pub const ASYNC_OPERATION_WITH_PROGRESS_COMPLETED_HANDLER: Guid =
        Guid::parse("e85df41d-6aa7-46e3-a8e2-f009d840c627");
    pub const EVENT_HANDLER: Guid = Guid::parse("9de1c535-6ae1-11e0-84e1-18a905bcc53f");
    pub const I_ASYNC_ACTION_WITH_PROGRESS: Guid =
        Guid::parse("1f6db258-e803-48a1-9546-eb7353398884");
    pub const I_ASYNC_OPERATION: Guid = Guid::parse("9fc2b0bb-e446-44e2-aa61-9cab8f636af2");
    pub const I_ASYNC_OPERATION_WITH_PROGRESS: Guid =
        Guid::parse("b5d036d7-e297-498f-ba60-0289e76e23dd");
    pub const I_ITERABLE: Guid = Guid::parse("faa585ea-6214-4217-afda-7f46de5869b3");
    pub const I_ITERATOR: Guid = Guid::parse("6a79e863-4300-459a-9966-cbb660963ee1");
    pub const I_KEY_VALUE_PAIR: Guid = Guid::parse("02b51929-c1c4-4a7e-8940-0312b5c18500");
    pub const I_MAP: Guid = Guid::parse("3c2925fe-8519-45c1-aa79-197b6718c1c1");
    pub const I_MAP_CHANGED_EVENT_ARGS: Guid = Guid::parse("9939f4df-050a-4c0f-aa60-77075f9c4777");
    pub const I_MAP_VIEW: Guid = Guid::parse("e480ce40-a338-4ada-adcf-272272e48cb9");
    pub const I_OBSERVABLE_MAP: Guid = Guid::parse("65df2bf5-bf39-41b5-aebc-5a9d865e472b");
    pub const I_OBSERVABLE_VECTOR: Guid = Guid::parse("5917eb53-50b4-4a0d-b309-65862b3f1dbc");
    pub const I_REFERENCE_ARRAY: Guid = Guid::parse("61c17707-2d65-11e0-9ae8-d48564015472");
    pub const I_VECTOR: Guid = Guid::parse("913337e9-11a1-4345-a3a2-4e7f956e222d");
    pub const I_VECTOR_VIEW: Guid = Guid::parse("bbe1fa4c-b0e3-4583-baef-1f1b2e483e56");
    pub const MAP_CHANGED_EVENT_HANDLER: Guid = Guid::parse("179517f3-94ee-41f8-bddc-768a895544f3");
    pub const TYPED_EVENT_HANDLER: Guid = Guid::parse("9de1c534-6ae1-11e0-84e1-18a905bcc53f");
    pub const VECTOR_CHANGED_EVENT_HANDLER: Guid =
        Guid::parse("0c051752-9fbf-4c70-aa0c-0e4c82d9a761");
}
