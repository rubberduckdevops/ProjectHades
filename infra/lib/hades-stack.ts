import * as fs from 'fs';
import * as path from 'path';
import {
  Annotations,
  CfnOutput,
  Duration,
  Fn,
  RemovalPolicy,
  Stack,
  StackProps,
} from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as cognito from 'aws-cdk-lib/aws-cognito';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as apigwv2 from 'aws-cdk-lib/aws-apigatewayv2';
import { HttpLambdaIntegration } from 'aws-cdk-lib/aws-apigatewayv2-integrations';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as s3deploy from 'aws-cdk-lib/aws-s3-deployment';
import * as cloudfront from 'aws-cdk-lib/aws-cloudfront';
import * as origins from 'aws-cdk-lib/aws-cloudfront-origins';
import { RustFunction } from 'cargo-lambda-cdk';
import { Construct } from 'constructs';

export class HadesStack extends Stack {
  constructor(scope: Construct, id: string, props?: StackProps) {
    super(scope, id, props);

    // ---- DynamoDB -------------------------------------------------------
    const table = new dynamodb.Table(this, 'JournalTable', {
      tableName: 'hades_journal',
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'sk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: true },
      removalPolicy: RemovalPolicy.RETAIN,
    });

    // ---- Cognito --------------------------------------------------------
    const userPool = new cognito.UserPool(this, 'UserPool', {
      userPoolName: 'hades-users',
      signInAliases: { email: true },
      selfSignUpEnabled: false,
      autoVerify: { email: true },
      accountRecovery: cognito.AccountRecovery.EMAIL_ONLY,
      removalPolicy: RemovalPolicy.RETAIN,
    });

    const userPoolClient = userPool.addClient('WebClient', {
      userPoolClientName: 'hades-web',
      generateSecret: false,
      authFlows: { userSrp: true, userPassword: true },
    });

    // ---- Notes bucket -----------------------------------------------------
    // Note content lives at users/<sub>/notes/<path>. This bucket is the source
    // of truth for notes, so it is versioned and retained on stack deletion
    // (no autoDeleteObjects).
    const notesBucket = new s3.Bucket(this, 'NotesBucket', {
      versioned: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy: RemovalPolicy.RETAIN,
      lifecycleRules: [
        {
          id: 'NotesVersionHygiene',
          enabled: true,
          noncurrentVersionExpiration: Duration.days(90),
          expiredObjectDeleteMarker: true,
          abortIncompleteMultipartUploadAfter: Duration.days(7),
        },
      ],
    });
    // Phase 2: S3 event notifications (ObjectCreated/ObjectRemoved on
    // users/*/notes/*) -> SQS queue + DLQ -> indexer Lambda, plus the S3 Vectors
    // bucket and index, go here.

    // ---- Lambda (Rust / Axum) -------------------------------------------
    const fn = new RustFunction(this, 'ApiFunction', {
      manifestPath: path.join(__dirname, '../../backend/Cargo.toml'),
      architecture: lambda.Architecture.ARM_64,
      memorySize: 256,
      timeout: Duration.seconds(10),
      environment: {
        TABLE_NAME: table.tableName,
        NOTES_BUCKET: notesBucket.bucketName,
        COGNITO_USER_POOL_ID: userPool.userPoolId,
        COGNITO_CLIENT_ID: userPoolClient.userPoolClientId,
        // AWS_REGION is set automatically by the Lambda runtime.
      },
    });
    // Includes PutItem/UpdateItem/DeleteItem/ConditionCheckItem, which is what
    // TransactWriteItems (notes move) is authorized against.
    table.grantReadWriteData(fn);
    // Object actions scoped to users/*: s3:GetObject* (incl. GetObjectVersion),
    // s3:PutObject, s3:DeleteObject* (incl. DeleteObjectVersion), s3:Abort*;
    // bucket-level s3:List* / s3:GetBucket* on the bucket ARN. CopyObject needs
    // GetObject on the source + PutObject on the destination, both covered.
    notesBucket.grantReadWrite(fn, 'users/*');

    // ---- HTTP API -------------------------------------------------------
    const httpApi = new apigwv2.HttpApi(this, 'HttpApi', {
      apiName: 'hades-api',
      // $default route -> Lambda. Auth is enforced in-app (Cognito JWT).
      defaultIntegration: new HttpLambdaIntegration('LambdaIntegration', fn),
    });

    // apiEndpoint looks like https://<id>.execute-api.<region>.amazonaws.com
    const apiDomain = Fn.select(2, Fn.split('/', httpApi.apiEndpoint));

    // ---- Frontend hosting ----------------------------------------------
    const siteBucket = new s3.Bucket(this, 'SiteBucket', {
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      enforceSSL: true,
      encryption: s3.BucketEncryption.S3_MANAGED,
      removalPolicy: RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
    });

    // SPA fallback: rewrite every non-file path to /index.html. Attached to
    // the S3 (default) behavior only so API 404s are not masked.
    const spaRewrite = new cloudfront.Function(this, 'SpaRewriteFunction', {
      runtime: cloudfront.FunctionRuntime.JS_2_0,
      code: cloudfront.FunctionCode.fromInline(`
function handler(event) {
  var request = event.request;
  var uri = request.uri;
  // Real files: Vite build output under /assets/, and top-level files such as
  // /config.json or /favicon.svg. Everything else is an SPA route, including
  // note URLs like /notes/Projects/Design.md that end in a file extension.
  var isAsset = uri.indexOf('/assets/') === 0;
  var isRootFile = uri.lastIndexOf('/') === 0 && uri.indexOf('.') !== -1;
  if (!isAsset && !isRootFile) {
    request.uri = '/index.html';
  }
  return request;
}
`),
    });

    const distribution = new cloudfront.Distribution(this, 'Distribution', {
      defaultRootObject: 'index.html',
      defaultBehavior: {
        origin: origins.S3BucketOrigin.withOriginAccessControl(siteBucket),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        functionAssociations: [
          {
            function: spaRewrite,
            eventType: cloudfront.FunctionEventType.VIEWER_REQUEST,
          },
        ],
      },
      additionalBehaviors: {
        '/api/*': {
          origin: new origins.HttpOrigin(apiDomain, {
            protocolPolicy: cloudfront.OriginProtocolPolicy.HTTPS_ONLY,
          }),
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
        },
      },
    });

    // ---- Frontend deployment --------------------------------------------
    const distDir = path.join(__dirname, '../../frontend/dist');
    if (fs.existsSync(distDir)) {
      new s3deploy.BucketDeployment(this, 'DeploySite', {
        destinationBucket: siteBucket,
        sources: [
          // A local-dev public/config.json would be copied into dist; never ship it.
          s3deploy.Source.asset(distDir, { exclude: ['config.json', 'config.example.json'] }),
          s3deploy.Source.jsonData('config.json', {
            apiUrl: '/api',
            region: this.region,
            userPoolId: userPool.userPoolId,
            userPoolClientId: userPoolClient.userPoolClientId,
          }),
        ],
        distribution,
        distributionPaths: ['/*'],
      });
    } else {
      Annotations.of(this).addWarning(
        `Frontend build not found at ${distDir}; skipping site deployment. ` +
          'Run `npm run build` in frontend/ and redeploy.',
      );
    }

    // ---- Outputs --------------------------------------------------------
    new CfnOutput(this, 'CloudFrontUrl', {
      value: `https://${distribution.distributionDomainName}`,
    });
    new CfnOutput(this, 'HttpApiUrl', { value: httpApi.apiEndpoint });
    new CfnOutput(this, 'TableName', { value: table.tableName });
    new CfnOutput(this, 'NotesBucketName', { value: notesBucket.bucketName });
    new CfnOutput(this, 'UserPoolId', { value: userPool.userPoolId });
    new CfnOutput(this, 'UserPoolClientId', { value: userPoolClient.userPoolClientId });
    new CfnOutput(this, 'Region', { value: this.region });
  }
}
