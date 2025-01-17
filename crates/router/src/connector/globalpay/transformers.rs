use common_utils::crypto::{self, GenerateDigest};
use error_stack::ResultExt;
use rand::distributions::DistString;
use serde::{Deserialize, Serialize};

use super::{
    requests::{self, GlobalpayPaymentsRequest, GlobalpayRefreshTokenRequest},
    response::{GlobalpayPaymentStatus, GlobalpayPaymentsResponse, GlobalpayRefreshTokenResponse},
};
use crate::{
    connector::utils::{self, RouterData, WalletData},
    consts,
    core::errors,
    services::{self},
    types::{self, api, storage::enums, ErrorResponse},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct GlobalPayMeta {
    account_name: String,
}

impl TryFrom<&types::PaymentsAuthorizeRouterData> for GlobalpayPaymentsRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::PaymentsAuthorizeRouterData) -> Result<Self, Self::Error> {
        let metadata: GlobalPayMeta =
            utils::to_connector_meta_from_secret(item.connector_meta_data.clone())?;
        let account_name = metadata.account_name;
        let payment_method_data = match item.request.payment_method_data.clone() {
            api::PaymentMethodData::Card(ccard) => {
                requests::PaymentMethodData::Card(requests::Card {
                    number: ccard.card_number,
                    expiry_month: ccard.card_exp_month,
                    expiry_year: ccard.card_exp_year,
                    cvv: ccard.card_cvc,
                    account_type: None,
                    authcode: None,
                    avs_address: None,
                    avs_postal_code: None,
                    brand_reference: None,
                    chip_condition: None,
                    cvv_indicator: Default::default(),
                    funding: None,
                    pin_block: None,
                    tag: None,
                    track: None,
                })
            }
            api::PaymentMethodData::Wallet(wallet_data) => match wallet_data {
                api_models::payments::WalletData::PaypalRedirect(_) => {
                    requests::PaymentMethodData::Apm(requests::Apm {
                        provider: Some(requests::ApmProvider::Paypal),
                    })
                }
                api_models::payments::WalletData::GooglePay(_) => {
                    requests::PaymentMethodData::DigitalWallet(requests::DigitalWallet {
                        provider: Some(requests::DigitalWalletProvider::PayByGoogle),
                        payment_token: wallet_data.get_wallet_token_as_json()?,
                    })
                }
                _ => Err(errors::ConnectorError::NotImplemented(
                    "Payment methods".to_string(),
                ))?,
            },
            _ => Err(errors::ConnectorError::NotImplemented(
                "Payment methods".to_string(),
            ))?,
        };
        Ok(Self {
            account_name,
            amount: Some(item.request.amount.to_string()),
            currency: item.request.currency.to_string(),
            reference: item.attempt_id.to_string(),
            country: item.get_billing_country()?,
            capture_mode: Some(requests::CaptureMode::from(item.request.capture_method)),
            payment_method: requests::PaymentMethod {
                payment_method_data,
                authentication: None,
                encryption: None,
                entry_mode: Default::default(),
                fingerprint_mode: None,
                first_name: None,
                id: None,
                last_name: None,
                name: None,
                narrative: None,
                storage_mode: None,
            },
            notifications: Some(requests::Notifications {
                return_url: item.request.complete_authorize_url.clone(),
                challenge_return_url: None,
                decoupled_challenge_return_url: None,
                status_url: item.request.webhook_url.clone(),
                three_ds_method_return_url: None,
            }),
            authorization_mode: None,
            cashback_amount: None,
            channel: Default::default(),
            convenience_amount: None,
            currency_conversion: None,
            description: None,
            device: None,
            gratuity_amount: None,
            initiator: None,
            ip_address: None,
            language: None,
            lodging: None,
            order: None,
            payer_reference: None,
            site_reference: None,
            stored_credential: None,
            surcharge_amount: None,
            total_capture_count: None,
            globalpay_payments_request_type: None,
            user_reference: None,
        })
    }
}

impl TryFrom<&types::PaymentsCaptureRouterData> for requests::GlobalpayCaptureRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(value: &types::PaymentsCaptureRouterData) -> Result<Self, Self::Error> {
        Ok(Self {
            amount: Some(value.request.amount_to_capture.to_string()),
        })
    }
}

impl TryFrom<&types::PaymentsCancelRouterData> for requests::GlobalpayCancelRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(value: &types::PaymentsCancelRouterData) -> Result<Self, Self::Error> {
        Ok(Self {
            amount: value.request.amount.map(|amount| amount.to_string()),
        })
    }
}

pub struct GlobalpayAuthType {
    pub app_id: String,
    pub key: String,
}

impl TryFrom<&types::ConnectorAuthType> for GlobalpayAuthType {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(auth_type: &types::ConnectorAuthType) -> Result<Self, Self::Error> {
        match auth_type {
            types::ConnectorAuthType::BodyKey { api_key, key1 } => Ok(Self {
                app_id: key1.to_string(),
                key: api_key.to_string(),
            }),
            _ => Err(errors::ConnectorError::FailedToObtainAuthType.into()),
        }
    }
}

impl TryFrom<GlobalpayRefreshTokenResponse> for types::AccessToken {
    type Error = error_stack::Report<errors::ParsingError>;

    fn try_from(item: GlobalpayRefreshTokenResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            token: item.token,
            expires: item.seconds_to_expire,
        })
    }
}

impl TryFrom<&types::RefreshTokenRouterData> for GlobalpayRefreshTokenRequest {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(item: &types::RefreshTokenRouterData) -> Result<Self, Self::Error> {
        let globalpay_auth = GlobalpayAuthType::try_from(&item.connector_auth_type)
            .change_context(errors::ConnectorError::FailedToObtainAuthType)
            .attach_printable("Could not convert connector_auth to globalpay_auth")?;

        let nonce = rand::distributions::Alphanumeric.sample_string(&mut rand::thread_rng(), 12);
        let nonce_with_api_key = format!("{}{}", nonce, globalpay_auth.key);
        let secret_vec = crypto::Sha512
            .generate_digest(nonce_with_api_key.as_bytes())
            .change_context(errors::ConnectorError::RequestEncodingFailed)
            .attach_printable("error creating request nonce")?;

        let secret = hex::encode(secret_vec);

        Ok(Self {
            app_id: globalpay_auth.app_id,
            nonce,
            secret,
            grant_type: "client_credentials".to_string(),
        })
    }
}

impl From<GlobalpayPaymentStatus> for enums::AttemptStatus {
    fn from(item: GlobalpayPaymentStatus) -> Self {
        match item {
            GlobalpayPaymentStatus::Captured | GlobalpayPaymentStatus::Funded => Self::Charged,
            GlobalpayPaymentStatus::Declined | GlobalpayPaymentStatus::Rejected => Self::Failure,
            GlobalpayPaymentStatus::Preauthorized => Self::Authorized,
            GlobalpayPaymentStatus::Reversed => Self::Voided,
            GlobalpayPaymentStatus::Initiated => Self::AuthenticationPending,
            GlobalpayPaymentStatus::Pending => Self::Pending,
        }
    }
}

impl From<GlobalpayPaymentStatus> for enums::RefundStatus {
    fn from(item: GlobalpayPaymentStatus) -> Self {
        match item {
            GlobalpayPaymentStatus::Captured | GlobalpayPaymentStatus::Funded => Self::Success,
            GlobalpayPaymentStatus::Declined | GlobalpayPaymentStatus::Rejected => Self::Failure,
            GlobalpayPaymentStatus::Initiated | GlobalpayPaymentStatus::Pending => Self::Pending,
            _ => Self::Pending,
        }
    }
}

impl From<Option<enums::CaptureMethod>> for requests::CaptureMode {
    fn from(capture_method: Option<enums::CaptureMethod>) -> Self {
        match capture_method {
            Some(enums::CaptureMethod::Manual) => Self::Later,
            _ => Self::Auto,
        }
    }
}

fn get_payment_response(
    status: enums::AttemptStatus,
    response: GlobalpayPaymentsResponse,
) -> Result<types::PaymentsResponseData, ErrorResponse> {
    let redirection_data = response.payment_method.as_ref().and_then(|payment_method| {
        payment_method.redirect_url.as_ref().map(|redirect_url| {
            services::RedirectForm::from((redirect_url.to_owned(), services::Method::Get))
        })
    });
    match status {
        enums::AttemptStatus::Failure => Err(ErrorResponse {
            message: response
                .payment_method
                .and_then(|pm| pm.message)
                .unwrap_or_else(|| consts::NO_ERROR_MESSAGE.to_string()),
            ..Default::default()
        }),
        _ => Ok(types::PaymentsResponseData::TransactionResponse {
            resource_id: types::ResponseId::ConnectorTransactionId(response.id),
            redirection_data,
            mandate_reference: None,
            connector_metadata: None,
        }),
    }
}

impl<F, T>
    TryFrom<types::ResponseRouterData<F, GlobalpayPaymentsResponse, T, types::PaymentsResponseData>>
    for types::RouterData<F, T, types::PaymentsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::ResponseRouterData<
            F,
            GlobalpayPaymentsResponse,
            T,
            types::PaymentsResponseData,
        >,
    ) -> Result<Self, Self::Error> {
        let status = enums::AttemptStatus::from(item.response.status);
        Ok(Self {
            status,
            response: get_payment_response(status, item.response),
            ..item.data
        })
    }
}

impl<F, T>
    TryFrom<types::ResponseRouterData<F, GlobalpayRefreshTokenResponse, T, types::AccessToken>>
    for types::RouterData<F, T, types::AccessToken>
{
    type Error = error_stack::Report<errors::ParsingError>;
    fn try_from(
        item: types::ResponseRouterData<F, GlobalpayRefreshTokenResponse, T, types::AccessToken>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            response: Ok(types::AccessToken {
                token: item.response.token,
                expires: item.response.seconds_to_expire,
            }),
            ..item.data
        })
    }
}

impl<F> TryFrom<&types::RefundsRouterData<F>> for requests::GlobalpayRefundRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::RefundsRouterData<F>) -> Result<Self, Self::Error> {
        Ok(Self {
            amount: item.request.refund_amount.to_string(),
        })
    }
}

impl TryFrom<types::RefundsResponseRouterData<api::Execute, GlobalpayPaymentsResponse>>
    for types::RefundExecuteRouterData
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::RefundsResponseRouterData<api::Execute, GlobalpayPaymentsResponse>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            response: Ok(types::RefundsResponseData {
                connector_refund_id: item.response.id,
                refund_status: enums::RefundStatus::from(item.response.status),
            }),
            ..item.data
        })
    }
}

impl TryFrom<types::RefundsResponseRouterData<api::RSync, GlobalpayPaymentsResponse>>
    for types::RefundsRouterData<api::RSync>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::RefundsResponseRouterData<api::RSync, GlobalpayPaymentsResponse>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            response: Ok(types::RefundsResponseData {
                connector_refund_id: item.response.id,
                refund_status: enums::RefundStatus::from(item.response.status),
            }),
            ..item.data
        })
    }
}

#[derive(Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct GlobalpayErrorResponse {
    pub error_code: String,
    pub detailed_error_code: String,
    pub detailed_error_description: String,
}
